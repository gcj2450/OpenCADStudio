//! IMPORT (the Import File window, or a file name on the command line,
//! routed by format) and DGNIMPORT / -DGNIMPORT (the DGN Import window or
//! the command line), and the import itself: a DGN model's layers, text
//! styles, cells and objects added as one undo step, into a new drawing
//! unless the window chose the current one.

use std::collections::HashMap;
use std::sync::Arc;

use super::*;
use crate::modules::insert::dgn_import::{self, DgnImportCommand, DgnImportRequest};
use crate::ui::window::import_dialogs::{
    unsupported_reason, DgnImportState, ImportDialogMsg, ImportFileState, Support, FORMATS,
};

/// How many files the Import File window offers as recent.
const RECENT: usize = 5;

/// Picks a file and registers its bytes (the web build reads them only
/// here); `None` when the picker is cancelled.
async fn pick(title: String, filters: Vec<(String, Vec<String>)>) -> Option<String> {
    let mut dialog = crate::sys::file_dialog().set_title(title.as_str());
    for (name, extensions) in &filters {
        let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(name.as_str(), &extensions);
    }
    let handle = dialog.pick_file().await?;
    let path = crate::sys::handle_path(&handle)
        .to_string_lossy()
        .into_owned();
    let bytes = handle.read().await;
    crate::scene::model::pdf_raster::register_source(&path, Arc::new(bytes));
    Some(path)
}

fn dgn_filters() -> Vec<(String, Vec<String>)> {
    vec![
        (
            "MicroStation DGN (*.dgn)".to_string(),
            vec!["dgn".to_string()],
        ),
        ("All DGN Files (*.*)".to_string(), vec!["*".to_string()]),
    ]
}

fn extension(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn exists(path: &str) -> bool {
    crate::scene::model::pdf_raster::source_bytes(path).is_some()
}

/// Whether the file is a DGN model DGNIMPORT reads.
fn is_dgn(path: &str) -> bool {
    crate::scene::model::underlay_vector::item_names(codec::entities::UnderlayType::Dgn, path)
        .and_then(|models| models.into_iter().next())
        .is_some_and(|model| {
            crate::scene::model::underlay_vector::dgn_import(path, &model).is_some()
        })
}

/// The importer a file goes to.
enum Route {
    Pdf,
    Dgn,
}

/// The importer for `path` chosen as `format` (an index of [`FORMATS`]; `None`
/// follows the extension), or why there is none.
fn route(path: &str, format: Option<usize>) -> Result<Route, String> {
    let f = format.map(|k| &FORMATS[k]);
    if let Some(f) = f {
        if let Some(reason) = unsupported_reason(f.support) {
            return Err(format!("{}: {reason}", f.name));
        }
    }
    let ext = extension(path);
    let by_extension = f.is_none_or(|f| f.support == Support::ByExtension);
    let pdf = if by_extension {
        ext == "pdf"
    } else {
        f.is_some_and(|f| f.name == "PDF Files")
    };
    if pdf {
        return Ok(Route::Pdf);
    }
    let dgn_format = f.is_some_and(|f| f.name.contains("DGN"));
    if (dgn_format || by_extension) && is_dgn(path) {
        return Ok(Route::Dgn);
    }
    if dgn_format || ext == "dgn" {
        return Err(crate::t!("Invalid or unsupported DGN file.").into_owned());
    }
    // Any other extension: the format whose patterns name it.
    let named = FORMATS
        .iter()
        .find(|f| f.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)));
    Err(
        match named.and_then(|f| unsupported_reason(f.support).map(|r| (f, r))) {
            Some((f, reason)) => format!("{}: {reason}", f.name),
            None => crate::t!("OpenCAD Studio has no importer for this format.").into_owned(),
        },
    )
}

impl OpenCADStudio {
    /// IMPORT, DGNIMPORT and -DGNIMPORT; `None` defers to the other
    /// families.
    pub(super) fn dispatch_import(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        let (verb, rest) = match cmd.trim().split_once(char::is_whitespace) {
            Some((verb, rest)) => (verb.to_ascii_uppercase(), rest.trim().trim_matches('"')),
            None => (cmd.trim().to_ascii_uppercase(), ""),
        };
        let dialog = crate::sys::file_dia();
        match (verb.as_str(), rest.is_empty()) {
            ("IMPORT", true) if dialog => {
                self.open_import_file_dialog();
                Some(Task::none())
            }
            ("IMPORT", true) => {
                use crate::command::{CadCommand, ValuePromptCommand};
                let command = ValuePromptCommand::new("_IMPORTFILE", "Enter import file name:");
                self.command_line.push_info(&command.prompt());
                self.tabs[i].active_cmd = Some(Box::new(command));
                Some(Task::none())
            }
            // A file named after IMPORT (or typed at its prompt).
            ("IMPORT" | "_IMPORTFILE", false) => Some(self.import_routed(rest, None)),
            ("_IMPORTFILE", true) => Some(Task::none()),
            ("DGNIMPORT", true) if dialog => Some(self.pick_dgn_file()),
            ("DGNIMPORT" | "-DGNIMPORT", _) => {
                use crate::command::CadCommand;
                let command = DgnImportCommand::new();
                self.command_line.push_info(&command.prompt());
                self.tabs[i].active_cmd = Some(Box::new(command));
                if !rest.is_empty() {
                    return Some(
                        self.feed_command(crate::command::StepInput::Text(rest.to_string())),
                    );
                }
                Some(Task::none())
            }
            _ => None,
        }
    }

    fn pick_dgn_file(&mut self) -> Task<Message> {
        let title = crate::t!("Import DGN File").into_owned();
        Task::perform(pick(title, dgn_filters()), |path| {
            Message::ImportDialog(ImportDialogMsg::DgnPicked(path))
        })
    }

    pub(in crate::app) fn open_import_file_dialog(&mut self) {
        let path = self
            .import_dialogs
            .recent
            .first()
            .cloned()
            .unwrap_or_default();
        self.import_dialogs.file = Some(ImportFileState {
            path,
            format: 0,
            search: String::new(),
            only_supported: false,
            error: None,
        });
        self.active_modal = Some(crate::app::ModalKind::ImportFile);
    }

    /// The DGN Import window for a file; an error when it is no DGN model.
    pub(in crate::app) fn open_dgn_import_dialog(&mut self, path: &str) -> Result<(), String> {
        use crate::scene::model::underlay_vector::{dgn_import, item_names};
        let models = item_names(codec::entities::UnderlayType::Dgn, path).unwrap_or_default();
        let Some(import) = models.first().and_then(|m| dgn_import(path, m)) else {
            return Err(crate::t!("Invalid or unsupported DGN file.").into_owned());
        };
        // The options chosen last stay when the file is changed.
        let previous = self.import_dialogs.dgn.take();
        let keep =
            |f: fn(&DgnImportState) -> bool, default: bool| previous.as_ref().map_or(default, f);
        self.import_dialogs.dgn = Some(DgnImportState {
            path: path.to_string(),
            version: if import.v7 { "V7" } else { "V8" },
            models: crate::ui::window::pdf_dialogs::item_thumbs(
                codec::entities::UnderlayType::Dgn,
                path,
            ),
            model: 0,
            current_drawing: keep(|s| s.current_drawing, false),
            prefix_duplicates: keep(|s| s.prefix_duplicates, false),
            sub_units: keep(|s| s.sub_units, false),
            explode_text_nodes: keep(|s| s.explode_text_nodes, false),
            units: import.unit_names.clone(),
            mapping_tab: previous.as_ref().map_or(0, |s| s.mapping_tab),
            mapping: [0, 1, 2, 3].map(|tab| dgn_import::mapping_rows(&import, tab)),
        });
        self.active_modal = Some(crate::app::ModalKind::DgnImport);
        Ok(())
    }

    fn remember_import(&mut self, path: &str) {
        let recent = &mut self.import_dialogs.recent;
        recent.retain(|p| p != path);
        recent.insert(0, path.to_string());
        recent.truncate(RECENT);
    }

    /// IMPORT on the command line: the file to its importer.
    fn import_routed(&mut self, path: &str, format: Option<usize>) -> Task<Message> {
        if !exists(path) {
            self.command_line
                .push_error(&format!("File {path} does not exist."));
            return Task::none();
        }
        match route(path, format) {
            Ok(Route::Pdf) => {
                self.remember_import(path);
                self.dispatch_command(&format!("_PDFIMPORTFILE {path}"))
            }
            Ok(Route::Dgn) => {
                self.remember_import(path);
                if crate::sys::file_dia() {
                    if let Err(error) = self.open_dgn_import_dialog(path) {
                        self.command_line.push_error(&error);
                    }
                    return Task::none();
                }
                self.dispatch_command(&format!("-DGNIMPORT {path}"))
            }
            Err(error) => {
                self.command_line.push_error(&error);
                Task::none()
            }
        }
    }

    pub(in crate::app) fn update_import_dialog(
        &mut self,
        message: ImportDialogMsg,
    ) -> Task<Message> {
        let i = self.active_tab;
        match message {
            ImportDialogMsg::Help(which) => {
                let help = match which {
                    "dgn" => crate::t!(
                        "Imports a DGN model as drawing objects: levels as layers, cells as blocks."
                    ),
                    _ => {
                        crate::t!("Imports a file of the chosen format with the format's importer.")
                    }
                };
                self.command_line.push_info(help.as_ref());
            }
            // ── Import File ──────────────────────────────────────────────
            ImportDialogMsg::Path(path) => {
                if let Some(s) = self.import_dialogs.file.as_mut() {
                    s.path = path;
                    s.error = None;
                }
            }
            ImportDialogMsg::Recent(n) => {
                let path = self.import_dialogs.recent.get(n).cloned();
                if let (Some(s), Some(path)) = (self.import_dialogs.file.as_mut(), path) {
                    s.path = path;
                    s.error = None;
                }
            }
            ImportDialogMsg::Format(k) => {
                if let Some(s) = self.import_dialogs.file.as_mut() {
                    s.format = k.min(FORMATS.len() - 1);
                    s.error = None;
                }
            }
            ImportDialogMsg::Search(text) => {
                if let Some(s) = self.import_dialogs.file.as_mut() {
                    s.search = text;
                }
            }
            ImportDialogMsg::OnlySupported(on) => {
                if let Some(s) = self.import_dialogs.file.as_mut() {
                    s.only_supported = on;
                }
            }
            ImportDialogMsg::Browse => {
                let Some(s) = self.import_dialogs.file.as_ref() else {
                    return Task::none();
                };
                let f = &FORMATS[s.format];
                let filters = vec![(
                    format!("{} ({})", f.name, f.patterns),
                    f.extensions.iter().map(|e| e.to_string()).collect(),
                )];
                let title = crate::t!("Import File").into_owned();
                return Task::perform(pick(title, filters), |path| {
                    Message::ImportDialog(ImportDialogMsg::Picked(path))
                });
            }
            ImportDialogMsg::Picked(Some(path)) => {
                if let Some(s) = self.import_dialogs.file.as_mut() {
                    s.path = path;
                    s.error = None;
                }
            }
            ImportDialogMsg::Picked(None) => {}
            ImportDialogMsg::ImportOk => {
                let Some(s) = self.import_dialogs.file.as_ref() else {
                    return Task::none();
                };
                let path = s.path.trim().trim_matches('"').to_string();
                let format = s.format;
                let error = if path.is_empty() {
                    Some(crate::t!("Filename cannot be blank").into_owned())
                } else if !exists(&path) {
                    Some(format!("File {path} does not exist."))
                } else {
                    None
                };
                let routed = match error {
                    Some(error) => Err(error),
                    None => route(&path, Some(format)),
                };
                match routed {
                    Err(error) => {
                        if let Some(s) = self.import_dialogs.file.as_mut() {
                            s.error = Some(error);
                        }
                    }
                    Ok(Route::Pdf) => {
                        self.remember_import(&path);
                        self.close_active_modal();
                        return self.dispatch_command(&format!("_PDFIMPORTFILE {path}"));
                    }
                    Ok(Route::Dgn) => {
                        self.remember_import(&path);
                        self.close_active_modal();
                        if let Err(error) = self.open_dgn_import_dialog(&path) {
                            self.command_line.push_error(&error);
                        }
                    }
                }
            }
            // ── DGN Import ───────────────────────────────────────────────
            ImportDialogMsg::DgnChange => return self.pick_dgn_file(),
            ImportDialogMsg::DgnPicked(Some(path)) => {
                if let Err(error) = self.open_dgn_import_dialog(&path) {
                    self.command_line.push_error(&error);
                }
            }
            ImportDialogMsg::DgnPicked(None) => {}
            ImportDialogMsg::DgnModel(k) => {
                use crate::scene::model::underlay_vector::dgn_import;
                if let Some(s) = self.import_dialogs.dgn.as_mut() {
                    if let Some(model) = s.models.get(k) {
                        if let Some(import) = dgn_import(&s.path, &model.label) {
                            s.mapping =
                                [0, 1, 2, 3].map(|tab| dgn_import::mapping_rows(&import, tab));
                            s.units = import.unit_names.clone();
                        }
                        s.model = k;
                    }
                }
            }
            ImportDialogMsg::DgnCurrent(on) => {
                if let Some(s) = self.import_dialogs.dgn.as_mut() {
                    s.current_drawing = on;
                }
            }
            ImportDialogMsg::DgnPrefix(on) => {
                if let Some(s) = self.import_dialogs.dgn.as_mut() {
                    s.prefix_duplicates = on;
                }
            }
            ImportDialogMsg::DgnSub(on) => {
                if let Some(s) = self.import_dialogs.dgn.as_mut() {
                    s.sub_units = on;
                }
            }
            ImportDialogMsg::DgnExplode(on) => {
                if let Some(s) = self.import_dialogs.dgn.as_mut() {
                    s.explode_text_nodes = on;
                }
            }
            ImportDialogMsg::DgnMappingTab(tab) => {
                if let Some(s) = self.import_dialogs.dgn.as_mut() {
                    s.mapping_tab = tab.min(3);
                }
            }
            ImportDialogMsg::DgnOk => {
                let Some(s) = self.import_dialogs.dgn.as_ref() else {
                    return Task::none();
                };
                let request = DgnImportRequest {
                    path: s.path.clone(),
                    model: s
                        .models
                        .get(s.model)
                        .map(|m| m.label.clone())
                        .unwrap_or_default(),
                    sub_units: s.sub_units,
                    explode_text_nodes: s.explode_text_nodes,
                    current_drawing: s.current_drawing,
                    prefix_duplicates: s.prefix_duplicates,
                };
                self.close_active_modal();
                return self.run_dgn_import(i, request);
            }
        }
        Task::none()
    }

    /// The import: into a new drawing (or the current one), one undo step.
    pub(in crate::app) fn run_dgn_import(
        &mut self,
        i: usize,
        request: DgnImportRequest,
    ) -> Task<Message> {
        let Some(import) =
            crate::scene::model::underlay_vector::dgn_import(&request.path, &request.model)
        else {
            self.command_line
                .push_error(crate::t!("Invalid or unsupported DGN file.").as_ref());
            return Task::none();
        };
        let converted = dgn_import::convert(&import, request.sub_units, request.explode_text_nodes);
        let mut task = Task::none();
        let i = if request.current_drawing {
            i
        } else {
            task = self.update(Message::TabNew);
            self.active_tab
        };
        self.push_undo_snapshot(i, "DGNIMPORT");
        let stem = std::path::Path::new(&request.path.replace('\\', "/"))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let scene = &mut self.tabs[i].scene;
        // In the current drawing, names it already has keep the existing
        // definitions, or are imported under the file's name as prefix.
        let prefix =
            (request.current_drawing && request.prefix_duplicates).then(|| format!("{stem}$0$"));
        let rename = |taken: bool, name: &str| match &prefix {
            Some(p) if taken && name != "0" => format!("{p}{name}"),
            _ => name.to_string(),
        };
        let mut layers: HashMap<String, String> = HashMap::new();
        for (name, color) in &converted.layers {
            let target = rename(scene.document.layers.contains(name), name);
            if !scene.document.layers.contains(&target) {
                let mut layer = codec::tables::Layer::new(target.as_str());
                layer.handle = scene.document.allocate_handle();
                layer.color = *color;
                let _ = scene.document.layers.add(layer);
            }
            layers.insert(name.clone(), target);
        }
        let mut styles: HashMap<String, String> = HashMap::new();
        for (name, font) in &converted.styles {
            let target = rename(scene.document.text_styles.contains(name), name);
            if !scene.document.text_styles.contains(&target) {
                let mut style = codec::tables::TextStyle::new(target.as_str());
                style.handle = scene.document.allocate_handle();
                style.font_file = font.clone();
                let _ = scene.document.text_styles.add(style);
            }
            styles.insert(name.clone(), target);
        }
        let mut blocks: HashMap<String, String> = HashMap::new();
        for (name, _) in &converted.blocks {
            let taken = scene.document.block_records.get(name).is_some();
            blocks.insert(name.clone(), rename(taken, name));
        }
        let fix = |mut entity: codec::EntityType| {
            let common = entity.common_mut();
            if let Some(layer) = layers.get(&common.layer) {
                common.layer = layer.clone();
            }
            match &mut entity {
                codec::EntityType::Text(t) => {
                    if let Some(s) = styles.get(&t.style) {
                        t.style = s.clone();
                    }
                }
                codec::EntityType::MText(t) => {
                    if let Some(s) = styles.get(&t.style) {
                        t.style = s.clone();
                    }
                }
                codec::EntityType::Insert(insert) => {
                    if let Some(b) = blocks.get(&insert.block_name) {
                        insert.block_name = b.clone();
                    }
                }
                _ => {}
            }
            entity
        };
        for (name, entities) in converted.blocks {
            let target = &blocks[&name];
            if scene.document.block_records.get(target).is_some() {
                continue;
            }
            let entities = entities.into_iter().map(&fix).collect();
            let _ = scene.define_block_from_owned_entities(entities, target, glam::DVec3::ZERO);
        }
        for entity in converted.entities {
            scene.add_entity(fix(entity));
        }
        if !request.current_drawing {
            scene.fit_all();
        }
        self.tabs[i].dirty = true;
        self.command_line.push_output("Regenerating model.");
        self.refresh_layer_panel();
        self.refresh_properties();
        task
    }
}
