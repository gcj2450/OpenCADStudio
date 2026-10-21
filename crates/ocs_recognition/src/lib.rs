//! Structural recognition from the live CAD database, with bounded work and
//! explicit source evidence. No exports, child processes or vision service.
//!
//! This first native reader supports planar centerlines and named symbols.
//! It reports unsupported geometry and ambiguous crossings for review; it does
//! not infer connectivity through equipment or across symbol gaps.
mod topology;

use codec::{
    entities::EntityType as E,
    types::{Matrix3, Matrix4, Transform, Vector3},
    CadDocument, Handle,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const OVERLAY_LAYER: &str = "CADAGENT_RECOGNITION";
const MAX_VISITS: usize = 20_000;
const MAX_REVIEW: usize = 500;
const MAX_SEGMENTS: usize = 2_000;

#[derive(Debug, Clone)]
pub struct Options {
    /// Case-insensitive pipe/evidence layer filter. Empty means all layers;
    /// callers should normally select their pipe centerline layers explicitly.
    pub layers: Vec<String>,
    pub tolerance: f64,
    /// Applied independently to each result collection.
    pub offset: usize,
    pub limit: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            layers: vec![],
            tolerance: 1.0,
            offset: 0,
            limit: 200,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Style {
    layer: String,
    color: String,
    linetype: String,
}
#[derive(Clone)]
struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    sources: Vec<String>,
    style: Style,
    bridge: bool,
}
fn xy(v: Vector3) -> [f64; 2] {
    [v.x, v.y]
}
fn h(h: Handle) -> String {
    format!("{:X}", h.value())
}
fn classify(name: &str) -> Option<&'static str> {
    let n = name.to_uppercase();
    if ["VALVE", "VLV", "阀"].iter().any(|s| n.contains(s)) {
        Some("valve")
    } else if ["FILTER", "STRAINER", "过滤"].iter().any(|s| n.contains(s)) {
        Some("filter")
    } else if ["REDUC", "异径", "变径"].iter().any(|s| n.contains(s)) {
        Some("reducer")
    } else if ["FLANGE", "法兰"].iter().any(|s| n.contains(s)) {
        Some("flange")
    } else if ["PUMP", "TANK", "BOILER", "EQUIP", "泵", "水箱"]
        .iter()
        .any(|s| n.contains(s))
    {
        Some("equipment")
    } else if ["ARROW", "FLOW", "流向"].iter().any(|s| n.contains(s)) {
        Some("arrow")
    } else {
        None
    }
}

struct Reader<'a> {
    doc: &'a CadDocument,
    options: &'a Options,
    blocks: HashMap<String, &'a codec::tables::BlockRecord>,
    members: HashMap<Handle, Vec<Handle>>,
    segments: Vec<Segment>,
    components: Vec<Value>,
    texts: Vec<Value>,
    review: Vec<Value>,
    inventory: BTreeMap<String, usize>,
    visits: usize,
    truncated: bool,
}
impl Reader<'_> {
    fn review(&mut self, item: Value) {
        if self.review.len() < MAX_REVIEW {
            self.review.push(item);
        } else {
            self.truncated = true;
        }
    }
    fn style(&self, e: &E, parent: &Style) -> Style {
        let c = e.common();
        let layer = if c.layer == "0" {
            parent.layer.clone()
        } else {
            c.layer.clone()
        };
        let record = self.doc.layers.get(&layer);
        let color = match c.color.index() {
            Some(0) => parent.color.clone(),
            Some(256) => record
                .map(|l| format!("{:?}", l.color))
                .unwrap_or_else(|| "ByLayer".into()),
            _ => format!("{:?}", c.color),
        };
        let linetype = if c.linetype.eq_ignore_ascii_case("BYBLOCK") {
            parent.linetype.clone()
        } else if c.linetype.eq_ignore_ascii_case("BYLAYER") {
            record
                .map(|l| l.line_type.clone())
                .unwrap_or_else(|| "CONTINUOUS".into())
        } else {
            c.linetype.clone()
        };
        Style {
            layer,
            color,
            linetype: linetype.to_uppercase(),
        }
    }
    fn selected(&self, layer: &str) -> bool {
        self.options.layers.is_empty()
            || self
                .options
                .layers
                .iter()
                .any(|l| l.eq_ignore_ascii_case(layer))
    }
    fn segment(&mut self, a: Vector3, b: Vector3, source: &str, style: &Style, bridge: bool) {
        if ![a.x, a.y, a.z, b.x, b.y, b.z]
            .iter()
            .all(|v| v.is_finite() && v.abs() < 1e12)
        {
            self.review(json!({"type":"invalid_coordinates","source":source}));
            return;
        }
        // The topology model is XY. Never flatten separate elevations into a
        // connection: unsupported elevations are explicit review evidence.
        if a.z.abs() > 1e-7 || b.z.abs() > 1e-7 {
            self.review(json!({"type":"non_planar_geometry","source":source}));
            return;
        }
        if topology::dist(xy(a), xy(b)) < 1e-9 {
            return;
        }
        if self.segments.len() >= MAX_SEGMENTS {
            self.truncated = true;
            return;
        }
        self.segments.push(Segment {
            a: xy(a),
            b: xy(b),
            sources: vec![source.into()],
            style: style.clone(),
            bridge,
        });
    }
    fn walk(
        &mut self,
        e: &E,
        transform: Transform,
        parent: &Style,
        path: &str,
        stack: &mut Vec<Handle>,
    ) {
        if self.visits >= MAX_VISITS {
            self.truncated = true;
            return;
        }
        self.visits += 1;
        let common = e.common();
        let style = self.style(e, parent);
        if style.layer.eq_ignore_ascii_case(OVERLAY_LAYER) || common.invisible {
            return;
        }
        let source = if path.is_empty() {
            h(common.handle)
        } else {
            format!("{path}/{}", h(common.handle))
        };
        let kind = match e {
            E::Line(_) => "LINE",
            E::LwPolyline(_) => "LWPOLYLINE",
            E::Insert(_) => "INSERT",
            E::Text(_) => "TEXT",
            E::MText(_) => "MTEXT",
            E::Arc(_) => "ARC",
            E::Circle(_) => "CIRCLE",
            _ => "OTHER",
        };
        *self.inventory.entry(kind.into()).or_default() += 1;
        let selected = self.selected(&style.layer);
        match e {
            E::Insert(ins)=>{
                let name=ins.block_name.to_uppercase();
                let Some(block)=self.blocks.get(&name).copied() else { self.review(json!({"type":"missing_block","source":source,"blockName":ins.block_name}));return; };
                if stack.len()>=24 || stack.contains(&block.handle) { self.review(json!({"type":"block_cycle_or_depth","source":source}));return; }
                if block.flags.is_xref || block.flags.is_external { self.review(json!({"type":"external_reference","source":source}));return; }
                if block.name.starts_with('*') && !block.name.to_uppercase().starts_with("*MODEL") {
                    self.review(json!({"type":"anonymous_block_visibility","source":source,"blockName":block.name}));
                    // Without evaluated visibility, expanding every member can
                    // invent hidden pipes. Leave the instance for inspection.
                    return;
                }
                let count=usize::from(ins.row_count.max(1))*usize::from(ins.column_count.max(1));
                if count>MAX_VISITS-self.visits { self.truncated=true;self.review(json!({"type":"array_budget","source":source,"instances":count}));return; }
                let rotation=Transform::from_matrix(Matrix4::rotation_z(ins.rotation));
                let orientation=rotation.then(&Transform::from_matrix(Matrix4::from_matrix3(Matrix3::arbitrary_axis(ins.normal))));
                let base=Transform::from_translation(-block.base_point)
                    .then(&Transform::from_scaling(Vector3::new(ins.x_scale(),ins.y_scale(),ins.z_scale())))
                    .then(&orientation);
                stack.push(block.handle);
                let children=self.members.get(&block.handle).cloned().unwrap_or_default();
                for row in 0..ins.row_count.max(1) { for col in 0..ins.column_count.max(1) {
                    if self.visits>=MAX_VISITS { self.truncated=true;break; }
                    self.visits+=1;
                    let offset=orientation.apply_rotation(Vector3::new(f64::from(col)*ins.column_spacing,f64::from(row)*ins.row_spacing,0.0));
                    let local=base.then(&Transform::from_translation(ins.insert_point+offset)).then(&transform);
                    let instance=if count==1 { source.clone() } else { format!("{source}[{row},{col}]") };
                    if let Some(kind)=classify(&ins.block_name) {
                        // A symbol on an excluded layer may contain strokes
                        // on a selected layer. Never expand those into pipes.
                        if !selected { continue; }
                        let attrs:BTreeMap<_,_>=ins.attributes.iter().map(|a|(a.tag.clone(),a.value.clone())).collect();
                        let tag=["TAG","ID","NUMBER","NO","编号"].iter().find_map(|k|attrs.get(*k)).cloned().unwrap_or_else(||ins.block_name.clone());
                        self.components.push(json!({"id":format!("c{}",self.components.len()),"source":instance,"handle":h(common.handle),"blockName":ins.block_name,"type":kind,"classificationEvidence":"block_name","tag":tag,"attributes":attrs,"position":xy(local.apply(block.base_point)),"layer":style.layer}));
                        self.review(json!({"type":"symbol_ports_require_review","source":instance,"componentType":kind}));
                        // Symbol strokes must never turn into pipe centerlines.
                        continue;
                    }
                    for child in &children {
                        if self.visits>=MAX_VISITS { self.truncated=true;break; }
                        if let Some(e)=self.doc.get_entity(*child) { self.walk(e,local,&style,&instance,stack); }
                    }
                }}
                stack.pop();
            }
            E::Line(line) if selected => self.segment(transform.apply(line.start),transform.apply(line.end),&source,&style,false),
            E::LwPolyline(poly) if selected => {
                if poly.normal!=Vector3::UNIT_Z { self.review(json!({"type":"unsupported_ocs","source":source}));return; }
                let n=poly.vertices.len();
                let count=n.saturating_sub(1)+usize::from(poly.is_closed && n>1);
                for i in 0..count.min(MAX_SEGMENTS+1) {
                    let a=&poly.vertices[i];let b=&poly.vertices[(i+1)%n];
                    self.segment(transform.apply(Vector3::new(a.location.x,a.location.y,poly.elevation)),transform.apply(Vector3::new(b.location.x,b.location.y,poly.elevation)),&format!("{source}:{i}"),&style,a.bulge.abs()>1e-9);
                }
                if count>MAX_SEGMENTS { self.truncated=true; }
            }
            E::Arc(arc) if selected => {
                if arc.normal!=Vector3::UNIT_Z { self.review(json!({"type":"unsupported_ocs","source":source}));return; }
                let at=|angle:f64|transform.apply(arc.center+Vector3::new(arc.radius*angle.cos(),arc.radius*angle.sin(),0.0));
                self.segment(at(arc.start_angle),at(arc.end_angle),&source,&style,true);
            }
            E::Text(t) if selected => self.texts.push(json!({"source":source,"text":t.value,"position":xy(transform.apply(t.insertion_point)),"layer":style.layer})),
            E::MText(t) if selected => self.texts.push(json!({"source":source,"text":codec::entities::mtext_format::parse_mtext(&t.value,true).to_plain_text(),"position":xy(transform.apply(t.insertion_point)),"layer":style.layer})),
            E::Circle(_) | E::Hatch(_) | E::Solid(_) | E::Point(_) | E::AttributeDefinition(_) | E::AttributeEntity(_) | E::Dimension(_) | E::Leader(_) | E::MultiLeader(_) | E::Block(_) | E::BlockEnd(_) | E::Seqend(_) => {},
            _ if selected => self.review(json!({"type":"unsupported_geometry","source":source,"entityType":kind})),
            _=>{}
        }
    }
}

/// Read model space only. Results are deterministic and individually paged;
/// `complete` means the supported structural pass stayed within its budget,
/// never that every domain-specific object has been understood.
pub fn recognize(doc: &CadDocument, options: &Options) -> Value {
    recognize_impl(doc, options, true)
}

/// Run recognition once and return every result collection without paging.
/// Hosts can retain this value for the current document revision and serve
/// follow-up pages without rebuilding topology.
pub fn recognize_all(doc: &CadDocument, options: &Options) -> Value {
    recognize_impl(doc, options, false)
}

fn recognize_impl(doc: &CadDocument, options: &Options, paged: bool) -> Value {
    if !options.tolerance.is_finite() || options.tolerance < 1e-9 {
        return json!({"ok":false,"error":"recognize: tolerance must be a finite drawing-unit distance of at least 1e-9"});
    }
    let blocks: HashMap<_, _> = doc
        .block_records
        .iter()
        .map(|b| (b.name.to_uppercase(), b))
        .collect();
    let mut members: HashMap<Handle, Vec<Handle>> = doc
        .block_records
        .iter()
        .map(|b| (b.handle, b.entity_handles.clone()))
        .collect();
    let mut owned = HashSet::new();
    for b in doc.block_records.iter() {
        if !b.name.to_uppercase().starts_with("*MODEL_SPACE") {
            owned.extend(b.entity_handles.iter().copied());
        }
    }
    for e in doc.entities() {
        let owner = e.common().owner_handle;
        if let Some(m) = members.get_mut(&owner) {
            m.push(e.common().handle);
        }
    }
    for m in members.values_mut() {
        m.sort_by_key(|h| h.value());
        m.dedup();
    }
    let mut reader = Reader {
        doc,
        options,
        blocks,
        members,
        segments: vec![],
        components: vec![],
        texts: vec![],
        review: vec![],
        inventory: BTreeMap::new(),
        visits: 0,
        truncated: false,
    };
    let mut roots: Vec<_> = doc
        .entities()
        .filter(|e| {
            let c = e.common();
            !owned.contains(&c.handle)
                && (c.owner_handle.is_null()
                    || c.owner_handle == doc.header.model_space_block_handle)
        })
        .collect();
    roots.sort_by_key(|e| e.common().handle.value());
    let default = Style {
        layer: "0".into(),
        color: "ByLayer".into(),
        linetype: "CONTINUOUS".into(),
    };
    for root in roots {
        if reader.visits >= MAX_VISITS {
            reader.truncated = true;
            break;
        }
        reader.walk(root, Transform::identity(), &default, "", &mut vec![]);
    }
    if options.layers.is_empty() {
        reader.review(json!({"type":"unfiltered_layers","message":"All supported linework is candidate geometry; select pipe centerline layers before treating it as pipes."}));
    }
    let (steps, nodes, crossings, topology_truncated) =
        topology::build(&reader.segments, options.tolerance);
    reader.truncated |= topology_truncated;
    for item in crossings {
        reader.review(item);
    }
    let mut result = json!({"ok":true,"schemaVersion":"ocs-recognition/1","engine":"native","layout":"model","drawingUnits":doc.header.insertion_units,"layers":options.layers,"tolerance":options.tolerance,"inventory":reader.inventory,
        "complete":!reader.truncated,"coverage":{"visits":reader.visits,"maxVisits":MAX_VISITS,"maxSegments":MAX_SEGMENTS,"unsupported":"See reviewItems; symbol ports, pipe labels and equipment-to-equipment paths require further inspection."},"page":{}});
    for (name, values) in [
        ("components", reader.components),
        ("texts", reader.texts),
        ("steps", steps),
        ("nodes", nodes),
        ("reviewItems", reader.review),
    ] {
        let total = values.len();
        let page: Vec<_> = if paged {
            values
                .into_iter()
                .skip(options.offset)
                .take(options.limit.clamp(1, 1000))
                .collect()
        } else {
            values
        };
        let next = if paged {
            options.offset.saturating_add(page.len())
        } else {
            total
        };
        result["page"][name] = json!({"total":total,"next_offset":(next<total).then_some(next)});
        result[name] = json!(page);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::{
        entities::{Insert, Line},
        tables::BlockRecord,
    };
    fn block(doc: &mut CadDocument, name: &str, base: Vector3) -> Handle {
        let mut b = BlockRecord::new(name);
        b.handle = doc.allocate_handle();
        b.base_point = base;
        let h = b.handle;
        doc.block_records.add(b).unwrap();
        h
    }
    fn line(doc: &mut CadDocument, owner: Handle, layer: &str, a: Vector3, b: Vector3) -> Handle {
        let mut e = Line::from_points(a, b);
        e.common.layer = layer.into();
        e.common.owner_handle = owner;
        doc.add_entity(E::Line(e)).unwrap()
    }
    #[test]
    fn nested_blocks_apply_base_scale_rotation_and_inherited_layer() {
        let mut doc = CadDocument::new();
        let inner = block(&mut doc, "Inner", Vector3::new(2., 0., 0.));
        let leaf = line(
            &mut doc,
            inner,
            "0",
            Vector3::new(2., 0., 0.),
            Vector3::new(3., 0., 0.),
        );
        let outer = block(&mut doc, "Outer", Vector3::ZERO);
        let mut nested = Insert::new("Inner", Vector3::new(5., 0., 0.));
        nested.common.owner_handle = outer;
        let nested = doc.add_entity(E::Insert(nested)).unwrap();
        let mut root = Insert::new("outer", Vector3::new(100., 100., 0.))
            .with_uniform_scale(2.)
            .with_rotation(std::f64::consts::FRAC_PI_2);
        root.common.layer = "PIPES".into();
        let root = doc.add_entity(E::Insert(root)).unwrap();
        let result = recognize(
            &doc,
            &Options {
                layers: vec!["pipes".into()],
                tolerance: 0.01,
                ..Default::default()
            },
        );
        assert_eq!(result["steps"].as_array().unwrap().len(), 1);
        let step = &result["steps"][0];
        assert!((step["start"][0].as_f64().unwrap() - 100.).abs() < 1e-9);
        assert!((step["start"][1].as_f64().unwrap() - 110.).abs() < 1e-9);
        assert!((step["end"][1].as_f64().unwrap() - 112.).abs() < 1e-9);
        assert_eq!(step["layer"], "PIPES");
        assert_eq!(
            step["sources"],
            json!([format!("{}/{}/{}", h(root), h(nested), h(leaf))])
        );
    }
    #[test]
    fn symbols_overlays_and_paper_space_are_excluded_from_pipes() {
        let mut doc = CadDocument::new();
        let valve = block(&mut doc, "VALVE", Vector3::ZERO);
        line(
            &mut doc,
            valve,
            "0",
            Vector3::ZERO,
            Vector3::new(20., 0., 0.),
        );
        let mut insert = Insert::new("VALVE", Vector3::ZERO);
        insert.common.layer = "PIPES".into();
        doc.add_entity(E::Insert(insert)).unwrap();
        line(
            &mut doc,
            Handle::NULL,
            OVERLAY_LAYER,
            Vector3::ZERO,
            Vector3::new(20., 0., 0.),
        );
        let paper = doc.header.paper_space_block_handle;
        line(
            &mut doc,
            paper,
            "PIPES",
            Vector3::ZERO,
            Vector3::new(20., 0., 0.),
        );
        let result = recognize(&doc, &Options::default());
        assert_eq!(result["steps"], json!([]));
        assert_eq!(result["components"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn excluded_symbol_strokes_do_not_leak_into_selected_layers() {
        let mut doc = CadDocument::new();
        let valve = block(&mut doc, "VALVE", Vector3::ZERO);
        line(&mut doc, valve, "PIPES", Vector3::ZERO, Vector3::new(10., 0., 0.));
        let mut insert = Insert::new("VALVE", Vector3::ZERO);
        insert.common.layer = "SYMBOLS".into();
        doc.add_entity(E::Insert(insert)).unwrap();
        let result = recognize(&doc, &Options { layers: vec!["PIPES".into()], ..Default::default() });
        assert_eq!(result["steps"], json!([]));
        assert_eq!(result["components"], json!([]));
    }

    #[test]
    fn cycles_and_result_pages_are_explicit() {
        let mut doc = CadDocument::new();
        let b = block(&mut doc, "Loop", Vector3::ZERO);
        let mut looped = Insert::new("Loop", Vector3::ZERO);
        looped.common.owner_handle = b;
        doc.add_entity(E::Insert(looped)).unwrap();
        doc.add_entity(E::Insert(Insert::new("Loop", Vector3::ZERO)))
            .unwrap();
        for i in 0..3 {
            line(
                &mut doc,
                Handle::NULL,
                "PIPES",
                Vector3::new(i as f64 * 100., 0., 0.),
                Vector3::new(i as f64 * 100. + 20., 0., 0.),
            );
        }
        let result = recognize(
            &doc,
            &Options {
                limit: 1,
                ..Default::default()
            },
        );
        assert_eq!(result["page"]["steps"]["total"], 3);
        assert_eq!(result["page"]["steps"]["next_offset"], 1);
        assert_eq!(result["reviewItems"][0]["type"], "block_cycle_or_depth");
        assert_eq!(
            recognize(
                &doc,
                &Options {
                    tolerance: 0.,
                    ..Default::default()
                }
            )["ok"],
            false
        );
    }
    #[test]
    fn array_spacing_is_rotated_but_not_scaled() {
        let mut doc = CadDocument::new();
        let b = block(&mut doc, "Centerline", Vector3::ZERO);
        line(&mut doc, b, "0", Vector3::ZERO, Vector3::new(1., 0., 0.));
        let mut root = Insert::new("Centerline", Vector3::ZERO)
            .with_uniform_scale(2.)
            .with_rotation(std::f64::consts::FRAC_PI_2);
        root.column_count = 2;
        root.column_spacing = 10.;
        doc.add_entity(E::Insert(root)).unwrap();
        let result = recognize(
            &doc,
            &Options {
                tolerance: 0.01,
                ..Default::default()
            },
        );
        assert_eq!(result["steps"].as_array().unwrap().len(), 2);
        assert!((result["steps"][1]["start"][1].as_f64().unwrap() - 10.).abs() < 1e-9);
        assert!(result["steps"][1]["sources"][0]
            .as_str()
            .unwrap()
            .contains("[0,1]"));
    }
}
