//! Fresh-water recognition integration for the OpenCADStudio desktop host.
//!
//! Recognition graphics are written to a dedicated layer so the layer can be
//! refreshed after the user edits the drawing or corrects a result.
use ocs_plugin_api::host::{codec, BuiltinPlugin, CadDocument, HostApi, LayerConfig};
use ocs_plugin_api::{
    ApiVersion, CadModule, IconKind, ModuleEvent, PluginManifest, RibbonGroup, RibbonItem, ToolDef,
};
use serde_json::Value;
use std::{
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
};

const RECOGNITION_LAYER: &str = "CADAGENT_RECOGNITION";
static MANIFEST: PluginManifest = PluginManifest {
    id: "cadagent.freshwater",
    name: "Fresh Water Recognition",
    version: "0.2.0",
    description: "Recognize fresh-water systems and overlay editable CAD graphics",
    api_version: ApiVersion::CURRENT,
    ribbon_order: 60,
    xdata_apps: &[],
    command_prefixes: &["FW_"],
};

pub fn export_document(doc: &CadDocument, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(output)?;
    serde_json::to_writer(
        BufWriter::new(fs::File::create(output.join("cad-document.json"))?),
        doc,
    )?;
    codec::io::dxf::DxfWriter::new(doc).write_to_file(output.join("drawing.dxf"))?;
    Ok(())
}

fn point(value: &Value) -> Option<codec::types::Vector3> {
    let a = value.as_array()?;
    Some(codec::types::Vector3::new(
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
    ))
}
fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}
fn line(start: codec::types::Vector3, end: codec::types::Vector3, color: i16) -> codec::EntityType {
    let mut entity = codec::EntityType::Line(codec::entities::Line::from_points(start, end));
    entity.common_mut().layer = RECOGNITION_LAYER.to_owned();
    entity.common_mut().color = codec::types::Color::from_index(color);
    entity
}
fn text(
    value: String,
    position: codec::types::Vector3,
    height: f64,
    color: i16,
) -> codec::EntityType {
    let mut entity = codec::EntityType::Text(
        codec::entities::Text::with_value(value, position).with_height(height.max(0.01)),
    );
    entity.common_mut().layer = RECOGNITION_LAYER.to_owned();
    entity.common_mut().color = codec::types::Color::from_index(color);
    entity
}
fn rectangle(min_x: f64, min_y: f64, max_x: f64, max_y: f64, color: i16) -> Vec<codec::EntityType> {
    let a = codec::types::Vector3::new(min_x, min_y, 0.0);
    let b = codec::types::Vector3::new(max_x, min_y, 0.0);
    let c = codec::types::Vector3::new(max_x, max_y, 0.0);
    let d = codec::types::Vector3::new(min_x, max_y, 0.0);
    vec![
        line(a, b, color),
        line(b, c, color),
        line(c, d, color),
        line(d, a, color),
    ]
}
fn component_graphics(component: &Value, text_height: f64) -> Vec<codec::EntityType> {
    let position = component
        .get("position")
        .and_then(point)
        .unwrap_or(codec::types::Vector3::ZERO);
    let kind = component
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("equipment")
        .to_ascii_lowercase();
    let color = match kind.as_str() {
        "valve" => 1,
        "reducer" => 5,
        "filter" => 6,
        _ => 3,
    };
    let bounds = component.get("bounds").and_then(Value::as_array);
    let (min_x, min_y, max_x, max_y) = bounds
        .and_then(|b| {
            Some((
                b.first()?.as_f64()?,
                b.get(1)?.as_f64()?,
                b.get(2)?.as_f64()?,
                b.get(3)?.as_f64()?,
            ))
        })
        .unwrap_or((
            position.x - 3.0,
            position.y - 2.0,
            position.x + 3.0,
            position.y + 2.0,
        ));
    let mut entities = match kind.as_str() {
        "valve" => {
            let r = ((max_x - min_x).abs().max((max_y - min_y).abs()) * 0.35).max(1.5);
            let p1 = codec::types::Vector3::new(position.x - r, position.y, 0.0);
            let p2 = codec::types::Vector3::new(position.x, position.y + r, 0.0);
            let p3 = codec::types::Vector3::new(position.x + r, position.y, 0.0);
            let p4 = codec::types::Vector3::new(position.x, position.y - r, 0.0);
            vec![
                line(p1, p2, color),
                line(p2, p3, color),
                line(p3, p4, color),
                line(p4, p1, color),
            ]
        }
        "reducer" => {
            let y0 = (min_y + max_y) * 0.5;
            let h = (max_y - min_y).abs().max(2.0) * 0.35;
            let a = codec::types::Vector3::new(min_x, y0 - h, 0.0);
            let b = codec::types::Vector3::new(min_x, y0 + h, 0.0);
            let c = codec::types::Vector3::new(max_x, y0 + h * 0.55, 0.0);
            let d = codec::types::Vector3::new(max_x, y0 - h * 0.55, 0.0);
            vec![
                line(a, b, color),
                line(b, c, color),
                line(c, d, color),
                line(d, a, color),
            ]
        }
        "filter" => {
            let mut e = rectangle(min_x, min_y, max_x, max_y, color);
            e.push(line(
                codec::types::Vector3::new(min_x, min_y, 0.0),
                codec::types::Vector3::new(max_x, max_y, 0.0),
                color,
            ));
            e
        }
        _ => rectangle(min_x, min_y, max_x, max_y, color),
    };
    let label = component
        .get("tag")
        .and_then(Value::as_str)
        .or_else(|| component.get("name").and_then(Value::as_str));
    if let Some(label) = label.filter(|s| !s.is_empty()) {
        entities.push(text(
            label.to_owned(),
            codec::types::Vector3::new(min_x, max_y + text_height * 0.5, 0.0),
            text_height,
            color,
        ));
    }
    entities
}
fn pipe_label(step: &Value) -> Option<String> {
    let id = step
        .get("number")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let diameter = step
        .get("diameter")
        .and_then(number)
        .map(|v| format!("Ø{}", v));
    let wall = step
        .get("wallThickness")
        .and_then(number)
        .map(|v| format!("t{}", v));
    [id.map(str::to_owned), diameter, wall]
        .into_iter()
        .flatten()
        .reduce(|a, b| format!("{a} {b}"))
}
fn overlay_entities(structured: &Value, text_height: f64) -> Vec<codec::EntityType> {
    let mut entities = Vec::new();
    if let Some(components) = structured.get("components").and_then(Value::as_array) {
        for component in components {
            entities.extend(component_graphics(component, text_height));
        }
    }
    if let Some(steps) = structured.get("steps").and_then(Value::as_array) {
        for step in steps {
            // Arc endpoints are connection evidence, not a straight centerline.
            if step["bridge"].as_bool() == Some(true) {
                continue;
            }
            let (Some(start), Some(end)) = (
                step.get("start").and_then(point),
                step.get("end").and_then(point),
            ) else {
                continue;
            };
            entities.push(line(start, end, 2));
            if let Some(label) = pipe_label(step) {
                entities.push(text(
                    label,
                    codec::types::Vector3::new(
                        (start.x + end.x) / 2.0,
                        (start.y + end.y) / 2.0 + text_height,
                        0.0,
                    ),
                    text_height,
                    2,
                ));
            }
        }
    }
    entities
}
fn make_output(root: &str, args: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if args.trim().is_empty() {
        Ok(Path::new(root).join("data/plugin-exports").join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        )))
    } else {
        Ok(Path::new(args.trim().trim_matches('"')).to_path_buf())
    }
}
fn refresh_overlay(
    host: &mut dyn HostApi,
    structured: &Value,
    text_height: f64,
) -> Result<usize, String> {
    let old: Vec<_> = host
        .document()
        .entities()
        .filter(|entity| {
            entity
                .common()
                .layer
                .eq_ignore_ascii_case(RECOGNITION_LAYER)
        })
        .map(|entity| entity.common().handle)
        .collect();
    for handle in old {
        host.remove_entity(handle);
    }
    let _ = host.add_layer(LayerConfig {
        name: RECOGNITION_LAYER.to_owned(),
        color: Some(codec::types::Color::from_index(2)),
        plottable: Some(false),
        description: Some("CAD Agent recognition overlay; refreshable".to_owned()),
        ..Default::default()
    });
    let entities = overlay_entities(structured, text_height);
    let count = entities.len();
    host.add_entities(entities);
    host.bump_geometry();
    host.set_dirty();
    Ok(count)
}

struct FreshWaterModule {
    groups: Vec<RibbonGroup>,
}
impl CadModule for FreshWaterModule {
    fn id(&self) -> &'static str {
        "freshwater"
    }
    fn title(&self) -> &'static str {
        "Fresh Water"
    }
    fn ribbon_groups(&self) -> &[RibbonGroup] {
        &self.groups
    }
}
#[derive(Default)]
struct FreshWaterPlugin;
impl BuiltinPlugin for FreshWaterPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(FreshWaterModule {
            groups: vec![RibbonGroup {
                title: "Recognition",
                tools: vec![
                    RibbonItem::LargeTool(ToolDef {
                        id: "FW_EXPORT",
                        label: "Export pipe connections",
                        icon: IconKind::Glyph("W"),
                        event: ModuleEvent::Command("FW_EXPORT".into()),
                    }),
                    RibbonItem::LargeTool(ToolDef {
                        id: "FW_RECOGNIZE",
                        label: "Recognize and overlay",
                        icon: IconKind::Glyph("R"),
                        event: ModuleEvent::Command("FW_RECOGNIZE".into()),
                    }),
                ],
            }],
        })
    }
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        let (verb, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
        if !verb.eq_ignore_ascii_case("FW_EXPORT") && !verb.eq_ignore_ascii_case("FW_RECOGNIZE") {
            return false;
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let options = ocs_recognition::Options {
                layers: vec![std::env::var("CAD_LAYER").unwrap_or_else(|_| "GSILayerP-TG".into())],
                tolerance: std::env::var("CAD_TOLERANCE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1.0),
                limit: 1000,
                ..Default::default()
            };
            // Run against the host document during this command. There is no
            // background snapshot that can be written into a different tab.
            let structured = ocs_recognition::recognize(host.document(), &options);
            if structured["ok"] != true {
                return Err(structured["error"]
                    .as_str()
                    .unwrap_or("native recognition failed")
                    .into());
            }
            if verb.eq_ignore_ascii_case("FW_EXPORT") || !args.trim().is_empty() {
                let root = std::env::var("CAD_AGENT_ROOT").unwrap_or_else(|_| ".".into());
                let output = make_output(&root, args)?;
                fs::create_dir_all(&output)?;
                if verb.eq_ignore_ascii_case("FW_EXPORT") {
                    export_document(host.document(), &output)?;
                }
                serde_json::to_writer_pretty(
                    BufWriter::new(fs::File::create(output.join("structured.json"))?),
                    &structured,
                )?;
                host.push_info(&format!(
                    "Native recognition results: {} (check complete and page cursors)",
                    output.display()
                ));
            }
            if verb.eq_ignore_ascii_case("FW_RECOGNIZE") {
                if structured["complete"] != true
                    || ["components", "steps"]
                        .iter()
                        .any(|k| !structured["page"][*k]["next_offset"].is_null())
                {
                    return Err("Recognition exceeded overlay budget; use native ocs_read recognize with layer filters and paging".into());
                }
                let height = std::env::var("CAD_OVERLAY_TEXT_HEIGHT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1.5);
                let count = refresh_overlay(host, &structured, height)?;
                host.push_info(&format!("Native recognition refreshed {count} CAD marks; symbol ports and review items need inspection"));
            }
            Ok(())
        })();
        if let Err(error) = result {
            host.push_error(&format!("Fresh water recognition failed: {error}"));
        }
        true
    }
}
ocs_plugin_api::export_plugin!(FreshWaterPlugin);
