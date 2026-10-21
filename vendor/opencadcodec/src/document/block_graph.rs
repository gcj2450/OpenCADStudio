//! Moving a block definition's object graph between drawings.
//!
//! A dynamic block keeps its parameters, grips and actions in an evaluation
//! graph owned by the extension dictionary of its block record, and names
//! itself through the record's XDATA. Copying only the record's entities
//! into another drawing leaves a plain block behind; these carry the graph
//! too, onto fresh handles.

use std::collections::{HashMap, HashSet};

use super::CadDocument;
use crate::classes::DxfClass;
use crate::objects::{BlockVisibilityParameter, ObjectType};
use crate::types::Handle;
use crate::xdata::{ExtendedData, XDataValue};

/// A block record's object graph lifted out of one drawing. See
/// [`CadDocument::export_block_graph`].
#[derive(Debug, Clone)]
pub struct BlockGraph {
    record: Handle,
    root: Option<Handle>,
    objects: Vec<(Handle, ObjectType)>,
    visibility: Vec<BlockVisibilityParameter>,
    representations: Vec<(Handle, Handle)>,
    xdata: Vec<(Handle, ExtendedData)>,
    classes: Vec<DxfClass>,
}

/// Visit every handle `object` refers to except its own. `false` for an
/// object kind whose references are not modelled, so it cannot be moved.
fn visit_references(object: &mut ObjectType, visit: &mut impl FnMut(&mut Handle)) -> bool {
    match object {
        ObjectType::Dictionary(d) => {
            visit(&mut d.owner);
            for (_, handle) in &mut d.entries {
                visit(handle);
            }
            d.reactors.iter_mut().for_each(&mut *visit);
            d.xdictionary_handle.iter_mut().for_each(visit);
        }
        ObjectType::DictionaryWithDefault(d) => {
            visit(&mut d.owner);
            for (_, handle) in &mut d.entries {
                visit(handle);
            }
            visit(&mut d.default_handle);
        }
        ObjectType::XRecord(x) => {
            visit(&mut x.owner);
            x.reactors.iter_mut().for_each(&mut *visit);
            x.xdictionary_handle.iter_mut().for_each(&mut *visit);
            for entry in &mut x.entries {
                if let crate::objects::XRecordValue::Handle(handle) = &mut entry.value {
                    visit(handle);
                }
            }
            for reference in &mut x.object_references {
                visit(&mut reference.handle);
            }
        }
        ObjectType::DynamicBlock(b) => b.visit_handles_mut(visit),
        ObjectType::BlockVisibilityParameter(p) => {
            let own = p.handle;
            p.visit_handles_mut(visit);
            p.handle = own;
        }
        _ => return false,
    }
    true
}

impl CadDocument {
    /// Lift block record `record`'s object graph: its extension dictionary
    /// and every object that dictionary owns, with the record's XDATA.
    /// `None` when there is nothing to carry, or when the graph holds an
    /// object kind that cannot be moved (the block then copies plain).
    pub fn export_block_graph(&self, record: Handle) -> Option<BlockGraph> {
        let root = self.extension_dictionary_handle(record);
        let mut objects = Vec::new();
        let mut seen = HashSet::new();
        let mut pending: Vec<Handle> = root.into_iter().collect();
        while let Some(handle) = pending.pop() {
            let owned = Some(handle) == root
                || root.is_some_and(|root| self.owner_chain_reaches(handle, root));
            if !seen.insert(handle) || !owned {
                continue;
            }
            let Some(object) = self.objects.get(&handle) else {
                continue;
            };
            let mut object = object.clone();
            if !visit_references(&mut object, &mut |child| pending.push(*child)) {
                return None;
            }
            objects.push((handle, object));
        }
        let mut xdata: Vec<(Handle, ExtendedData)> = std::iter::once(record)
            .chain(objects.iter().map(|(handle, _)| *handle))
            .filter_map(|handle| Some((handle, self.object_xdata.get(&handle)?.clone())))
            .collect();
        for (_, data) in &mut xdata {
            data.raw_dwg_eed.clear();
        }
        if objects.is_empty() && xdata.is_empty() {
            return None;
        }
        let visibility = objects
            .iter()
            .filter_map(|(handle, _)| self.block_visibility_params.get(handle).cloned())
            .collect();
        let representations = objects
            .iter()
            .filter_map(|(handle, _)| Some((*handle, *self.block_representations.get(handle)?)))
            .collect();
        let classes = objects
            .iter()
            .filter_map(|(_, object)| match object {
                ObjectType::DynamicBlock(b) => Some(b.dxf_name.as_str()),
                ObjectType::BlockVisibilityParameter(_) => Some("BLOCKVISIBILITYPARAMETER"),
                _ => None,
            })
            .filter_map(|name| self.classes.get_by_name(name).cloned())
            .collect();
        Some(BlockGraph {
            record,
            root,
            objects,
            visibility,
            representations,
            xdata,
            classes,
        })
    }

    /// Recreate `graph` on block record `record` of this drawing, on fresh
    /// handles. `map` sends the source handles the graph names outside
    /// itself (the definition's members) to their copies here; a reference
    /// neither in the graph nor in `map` is kept as it was.
    pub fn import_block_graph(
        &mut self,
        graph: &BlockGraph,
        record: Handle,
        map: &HashMap<Handle, Handle>,
    ) {
        let mut remap = map.clone();
        remap.insert(graph.record, record);
        for (handle, _) in &graph.objects {
            let fresh = self.allocate_handle();
            remap.insert(*handle, fresh);
        }
        let mut rewrite = |handle: &mut Handle| {
            if let Some(mapped) = remap.get(handle) {
                *handle = *mapped;
            }
        };
        for (handle, object) in &graph.objects {
            let mut object = object.clone();
            object.set_handle(remap[handle]);
            visit_references(&mut object, &mut rewrite);
            if let ObjectType::XRecord(x) = &mut object {
                // The source's DWG bytes name the source's handles.
                x.raw_dwg_data = None;
                x.raw_dwg_handle_bits = 0;
                x.raw_dwg_version = None;
                x.preserve_object_reference_stream = false;
            }
            self.objects.insert(remap[handle], object);
        }
        for parameter in &graph.visibility {
            let mut parameter = parameter.clone();
            parameter.visit_handles_mut(&mut rewrite);
            self.block_visibility_params.insert(parameter.handle, parameter);
        }
        for (data, definition) in &graph.representations {
            let mut definition = *definition;
            rewrite(&mut definition);
            self.block_representations.insert(remap[data], definition);
        }
        for (handle, data) in &graph.xdata {
            let mut copy = ExtendedData::new();
            for record in data.records() {
                let mut record = record.clone();
                for value in &mut record.values {
                    if let XDataValue::Handle(handle) = value {
                        rewrite(handle);
                    }
                }
                if !self.app_ids.contains(&record.application_name) {
                    let mut app = crate::tables::AppId::new(&record.application_name);
                    app.handle = self.allocate_handle();
                    let _ = self.app_ids.add(app);
                }
                copy.add_record(record);
            }
            self.object_xdata.insert(remap[handle], copy);
        }
        if let Some(root) = graph.root {
            self.xdic_by_handle.insert(record, remap[&root]);
        }
        for class in &graph.classes {
            if !self.classes.contains(&class.dxf_name) {
                self.classes.add_or_update(class.clone());
            }
        }
    }
}
