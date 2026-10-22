//! Domain-specific ship duct recognition.
//!
//! The layer classifier follows AI_Ship's LayerClassifier. Geometry is read
//! from the native document and every result retains source handles for review.

use super::{recognize_all, Options};
use codec::CadDocument;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_HIGHLIGHT_HANDLES: usize = 2_000;
const ZH_ANNOTATION: &str = "\u{6807}\u{6ce8}";
const ZH_CENTER: &str = "\u{4e2d}\u{5fc3}";
const ZH_END: &str = "\u{7aef}";
const ZH_END_LINE: &str = "\u{7aef}\u{7ebf}";
const ZH_FLANGE: &str = "\u{6cd5}\u{5170}";
const ZH_ELECTRIC: &str = "\u{7535}\u{52a8}";
const ZH_AIR: &str = "\u{98ce}\u{53e3}";
const ZH_VERTICAL: &str = "\u{7acb}\u{7ba1}";
const ZH_VALVE: &str = "\u{9600}";
const ZH_FIRE: &str = "\u{6d88}\u{9632}";
const ZH_PIPE: &str = "\u{7ba1}";
const ZH_SINGLE: &str = "\u{5355}\u{7ebf}";
const ZH_SIZE: &str = "\u{5c3a}\u{5bf8}";

#[derive(Debug, Clone)]
pub struct DuctOptions {
    pub pipe_layers: Vec<String>,
    pub centerline_layers: Vec<String>,
    pub annotation_layers: Vec<String>,
    pub tolerance: f64,
    pub corrections: Value,
    pub finalize: bool,
}

impl Default for DuctOptions {
    fn default() -> Self {
        Self {
            pipe_layers: vec![],
            centerline_layers: vec![],
            annotation_layers: vec![],
            tolerance: 1.0,
            corrections: Value::Array(vec![]),
            finalize: false,
        }
    }
}

fn root(name: &str) -> String {
    let suffixes = [
        "\u{5c3a}\u{5bf8}\u{6807}\u{6ce8}",
        ZH_ANNOTATION,
        "\u{7535}\u{52a8}\u{9600}\u{95e8}",
        "\u{9600}\u{95e8}-\u{7535}\u{52a8}",
        "\u{9600}\u{95e8}-\u{6d88}\u{9632}",
        ZH_VALVE,
        ZH_FLANGE,
        ZH_AIR,
        ZH_END_LINE,
        ZH_SINGLE,
        "\u{4e2d}\u{5fc3}\u{7ebf}",
        ZH_VERTICAL,
        "\u{8bbe}\u{5907}\u{6807}\u{6ce8}",
        "\u{8bbe}\u{5907}",
        ZH_PIPE,
        "FM_E",
        "FM_F",
        "FM",
        "F2",
        "FRZX",
        "ZX",
        "FL",
        "BZ",
        "FK",
        "SYMB",
        "LEAD",
        "DIM",
        "TEXT",
        ZH_SIZE,
    ];
    let mut end = name.len();
    loop {
        let candidate = &name[..end];
        let found = suffixes
            .iter()
            .filter_map(|suffix| {
                if candidate.ends_with(suffix) {
                    Some(candidate.len() - suffix.len())
                } else {
                    None
                }
            })
            .max();
        let Some(start) = found else { break };
        end = if start > 0
            && (name.as_bytes()[start - 1] == b'_' || name.as_bytes()[start - 1] == b'-')
        {
            start - 1
        } else {
            start
        };
    }
    let result = name[..end].trim_end_matches(['-', '_']).to_string();
    if result.trim().is_empty() {
        name.to_string()
    } else {
        result
    }
}

fn contains_any(text: &str, values: &[&str]) -> bool {
    values.iter().any(|value| text.contains(value))
}

fn category(name: &str, description: &str) -> &'static str {
    let text = format!("{name}|{description}").to_uppercase();
    if contains_any(&text, &[ZH_ANNOTATION, "DIM", "BZ", ZH_SIZE]) {
        "annotation"
    } else if contains_any(&text, &[ZH_CENTER, "ZX"]) {
        "centerline"
    } else if contains_any(&text, &[ZH_END_LINE, ZH_END]) {
        "endline"
    } else if contains_any(&text, &[ZH_FLANGE, "FL"]) {
        "flange"
    } else if contains_any(&text, &[ZH_ELECTRIC, "FM_E"]) {
        "electric_valve"
    } else if contains_any(&text, &[ZH_AIR, "FK"]) {
        "air_terminal"
    } else if text.contains(ZH_VERTICAL) {
        "vertical_pipe"
    } else if contains_any(&text, &[ZH_VALVE, "FM", "F2", ZH_FIRE]) {
        "valve"
    } else if text.contains(ZH_PIPE)
        || text.contains(ZH_SINGLE)
        || text.contains("PIPE")
        || text.contains("DUCT")
        || name.to_ascii_uppercase().ends_with("FG")
    {
        "pipe"
    } else {
        "other"
    }
}

/// Classify layers using AI_Ship's root stripping, keyword priority, and prefix merging rules.
pub fn classify_duct_layers(doc: &CadDocument) -> Value {
    let mut raw: Vec<(String, String, &'static str)> = doc
        .layers
        .iter()
        .filter(|layer| !layer.flags.xref_dependent && !layer.name.contains('|'))
        .map(|layer| {
            let system_root = root(&layer.name);
            (
                layer.name.clone(),
                system_root.clone(),
                category(&layer.name, &layer.description),
            )
        })
        .collect();
    raw.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    let mut roots: Vec<String> = raw.iter().map(|item| item.1.clone()).collect();
    roots.sort_by_key(|name| (name.len(), name.to_ascii_lowercase()));
    roots.dedup();
    let mut merged = HashMap::new();
    for name in &roots {
        let parent = roots.iter().find(|parent| {
            parent.len() < name.len()
                && (name.starts_with(&format!("{parent}-"))
                    || name.starts_with(&format!("{parent}_")))
        });
        let final_name = parent
            .and_then(|p| merged.get(p))
            .cloned()
            .unwrap_or_else(|| name.clone());
        merged.insert(name.clone(), final_name);
    }
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    for (name, raw_root, kind) in raw {
        let system = merged.get(&raw_root).cloned().unwrap_or(raw_root);
        let group = groups.entry(system.clone()).or_insert_with(|| json!({"systemName": system, "pipeLayers": [], "centerlineLayers": [], "endLineLayers": [], "flangeLayers": [], "valveLayers": [], "electricValveLayers": [], "annotationLayers": [], "airTerminalLayers": [], "verticalPipeLayers": [], "otherLayers": []}));
        let field = match kind {
            "pipe" => "pipeLayers",
            "centerline" => "centerlineLayers",
            "endline" => "endLineLayers",
            "flange" => "flangeLayers",
            "valve" => "valveLayers",
            "electric_valve" => "electricValveLayers",
            "annotation" => "annotationLayers",
            "air_terminal" => "airTerminalLayers",
            "vertical_pipe" => "verticalPipeLayers",
            _ => "otherLayers",
        };
        group[field]
            .as_array_mut()
            .expect("layer list")
            .push(json!(name));
    }
    json!({"ok": true, "engine": "native-duct", "systems": groups.into_values().collect::<Vec<_>>()})
}

fn node_angle(node: &Value, steps: &HashMap<String, &Value>) -> Option<f64> {
    let ids = node["steps"].as_array()?;
    if ids.len() != 2 {
        return None;
    }
    let point = [
        node["position"].get(0)?.as_f64()?,
        node["position"].get(1)?.as_f64()?,
    ];
    let direction = |step: &Value| -> Option<[f64; 2]> {
        let a = [
            step["start"].get(0)?.as_f64()?,
            step["start"].get(1)?.as_f64()?,
        ];
        let b = [step["end"].get(0)?.as_f64()?, step["end"].get(1)?.as_f64()?];
        let v = if (a[0] - point[0]).hypot(a[1] - point[1]) < 1e-7 {
            [b[0] - point[0], b[1] - point[1]]
        } else {
            [a[0] - point[0], a[1] - point[1]]
        };
        let length = v[0].hypot(v[1]);
        (length > 1e-9).then_some([v[0] / length, v[1] / length])
    };
    let a = direction(steps.get(ids[0].as_str()?)?)?;
    let b = direction(steps.get(ids[1].as_str()?)?)?;
    Some(
        (a[0] * b[0] + a[1] * b[1])
            .clamp(-1., 1.)
            .acos()
            .to_degrees(),
    )
}

fn entity_handles_for_layers(doc: &CadDocument, layers: &[String]) -> Vec<String> {
    let selected: HashSet<String> = layers.iter().map(|layer| layer.to_lowercase()).collect();
    let mut handles: Vec<String> = doc
        .entities()
        .filter_map(|entity| {
            let common = entity.common();
            selected
                .contains(&common.layer.to_lowercase())
                .then(|| format!("{:X}", common.handle.value()))
        })
        .take(MAX_HIGHLIGHT_HANDLES + 1)
        .collect();
    handles.sort();
    handles.dedup();
    handles.truncate(MAX_HIGHLIGHT_HANDLES);
    handles
}

#[derive(Clone)]
struct Face {
    nodes: Vec<String>,
    steps: Vec<String>,
    points: Vec<[f64; 2]>,
    area: f64,
    center: [f64; 2],
}

fn extract_faces(base: &Value) -> Vec<Face> {
    let nodes: HashMap<String, [f64; 2]> = base["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            Some((
                n["id"].as_str()?.to_string(),
                [n["position"][0].as_f64()?, n["position"][1].as_f64()?],
            ))
        })
        .collect();
    let mut adjacency: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for step in base["steps"].as_array().into_iter().flatten() {
        let (Some(id), Some(a), Some(b)) = (
            step["id"].as_str(),
            step["startNode"].as_str(),
            step["endNode"].as_str(),
        ) else {
            continue;
        };
        adjacency
            .entry(a.into())
            .or_default()
            .push((b.into(), id.into()));
        adjacency
            .entry(b.into())
            .or_default()
            .push((a.into(), id.into()));
    }
    for (node, neighbors) in &mut adjacency {
        if let Some(origin) = nodes.get(node) {
            neighbors.sort_by(|a, b| {
                let angle = |id: &str| {
                    nodes
                        .get(id)
                        .map(|p| (p[1] - origin[1]).atan2(p[0] - origin[0]))
                        .unwrap_or(0.0)
                };
                angle(&a.0).total_cmp(&angle(&b.0))
            });
        }
    }
    let mut visited = HashSet::<(String, String)>::new();
    let mut faces = Vec::new();
    let max_walk = adjacency.values().map(Vec::len).sum::<usize>();
    for (from, neighbors) in &adjacency {
        for (to, _) in neighbors {
            if visited.contains(&(from.clone(), to.clone())) {
                continue;
            }
            let (mut a, mut b) = (from.clone(), to.clone());
            let mut ring = Vec::new();
            let mut edge_ids = Vec::new();
            for _ in 0..max_walk {
                if !visited.insert((a.clone(), b.clone())) {
                    break;
                }
                ring.push(a.clone());
                if let Some((_, step)) = adjacency.get(&a).and_then(|v| v.iter().find(|x| x.0 == b))
                {
                    edge_ids.push(step.clone());
                }
                let Some(outgoing) = adjacency.get(&b) else {
                    break;
                };
                let Some(reverse) = outgoing.iter().position(|x| x.0 == a) else {
                    break;
                };
                let next = (reverse + outgoing.len() - 1) % outgoing.len();
                (a, b) = (b, outgoing[next].0.clone());
                if a == *from && b == *to {
                    break;
                }
            }
            if ring.len() < 3 || a != *from || b != *to {
                continue;
            }
            let points: Vec<_> = ring.iter().filter_map(|n| nodes.get(n).copied()).collect();
            if points.len() != ring.len() {
                continue;
            }
            let twice_area: f64 = points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let q = points[(i + 1) % points.len()];
                    p[0] * q[1] - q[0] * p[1]
                })
                .sum();
            if twice_area <= 1e-8 {
                continue;
            }
            let area = twice_area * 0.5;
            let center = [
                points.iter().map(|p| p[0]).sum::<f64>() / points.len() as f64,
                points.iter().map(|p| p[1]).sum::<f64>() / points.len() as f64,
            ];
            edge_ids.sort();
            edge_ids.dedup();
            faces.push(Face {
                nodes: ring,
                steps: edge_ids,
                points,
                area,
                center,
            });
        }
    }
    faces
}

fn parse_duct_size(text: &str) -> Option<[f64; 2]> {
    let normalized = text.replace('×', "x").replace('*', "x").replace('X', "x");
    let (left, right) = normalized.split_once('x')?;
    fn number(s: &str) -> Option<f64> {
        let start = s
            .char_indices()
            .find(|(_, c)| c.is_ascii_digit() || *c == '.')
            .map(|x| x.0)?;
        let token: String = s[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let value = token.parse::<f64>().ok()?;
        (value.is_finite() && value > 0.0).then_some(value)
    }
    Some([number(left)?, number(right)?])
}

fn distance_to_face(point: [f64; 2], face: &Face) -> f64 {
    face.points
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = face.points[(i + 1) % face.points.len()];
            let v = [b[0] - a[0], b[1] - a[1]];
            let d = v[0] * v[0] + v[1] * v[1];
            let t = if d > 0.0 {
                (((point[0] - a[0]) * v[0] + (point[1] - a[1]) * v[1]) / d).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (point[0] - a[0] - t * v[0]).hypot(point[1] - a[1] - t * v[1])
        })
        .fold(f64::INFINITY, f64::min)
}

/// Build a reviewable duct result. Corrections are applied only to the result;
/// the drawing remains unchanged. Set `finalize` after user review.
pub fn recognize_duct(doc: &CadDocument, options: &DuctOptions) -> Value {
    if !options.tolerance.is_finite() || options.tolerance < 1e-9 {
        return json!({"ok": false, "error": "duct_recognize: tolerance must be a finite drawing-unit distance of at least 1e-9"});
    }
    if options.pipe_layers.is_empty() {
        return json!({"ok": false, "error": "Confirm at least one duct pipe layer returned by duct_layers before recognition."});
    }
    let mut layers = options.pipe_layers.clone();
    layers.extend(options.centerline_layers.clone());
    layers.extend(options.annotation_layers.clone());
    layers.sort_by_key(|layer| layer.to_lowercase());
    layers.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let base = recognize_all(
        doc,
        &Options {
            layers: layers.clone(),
            tolerance: options.tolerance,
            ..Default::default()
        },
    );
    if base["ok"] != true {
        return base;
    }
    let source_steps: HashMap<String, &Value> = base["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|step| step["id"].as_str().map(|id| (id.to_string(), step)))
        .collect();
    let pipe_set: HashSet<String> = options
        .pipe_layers
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let center_set: HashSet<String> = options
        .centerline_layers
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let annotation_set: HashSet<String> = options
        .annotation_layers
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let nodes = base["nodes"].as_array().cloned().unwrap_or_default();
    let pipe_nodes: Vec<(String, [f64; 2])> = nodes
        .iter()
        .filter(|node| {
            node["layer"]
                .as_str()
                .is_some_and(|layer| pipe_set.contains(&layer.to_lowercase()))
        })
        .filter_map(|node| {
            Some((
                node["id"].as_str()?.to_string(),
                [
                    node["position"].get(0)?.as_f64()?,
                    node["position"].get(1)?.as_f64()?,
                ],
            ))
        })
        .collect();
    let mut junction_candidates = Vec::new();
    let mut vertical_evidence = Vec::new();
    let mut reviews = base["reviewItems"].as_array().cloned().unwrap_or_default();
    for node in &nodes {
        let Some(layer) = node["layer"].as_str() else {
            continue;
        };
        let degree = node["degree"].as_u64().unwrap_or(0) as usize;
        if center_set.contains(&layer.to_lowercase()) {
            if degree == 2 {
                let angle = node_angle(node, &source_steps).unwrap_or(0.0);
                let dashed = node["steps"].as_array().into_iter().flatten().any(|id| {
                    source_steps
                        .get(id.as_str().unwrap_or_default())
                        .and_then(|s| s["linetype"].as_str())
                        .is_some_and(|name| {
                            let upper = name.to_ascii_uppercase();
                            upper.contains("DASH")
                                || upper.contains("HIDDEN")
                                || upper.contains("CENTER")
                        })
                });
                let center = [
                    node["position"][0].as_f64().unwrap_or_default(),
                    node["position"][1].as_f64().unwrap_or_default(),
                ];
                let connected_corner = pipe_nodes
                    .iter()
                    .map(|(id, position)| {
                        (id, (position[0] - center[0]).hypot(position[1] - center[1]))
                    })
                    .filter(|(_, distance)| *distance <= options.tolerance)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if dashed && angle > 90.0 && angle < 180.0 && connected_corner.is_some() {
                    let id = node["id"].as_str().unwrap_or("node");
                    let (pipe_node, corner_distance) = connected_corner.expect("checked above");
                    vertical_evidence.push(json!({"id": format!("vertical-{id}"), "type": "vertical_duct_candidate", "centerlineNode": id, "connectedPipeNode": pipe_node, "pipeCornerDistance": corner_distance, "position": node["position"], "turnAngleDegrees": angle, "linetypeEvidence": "dashed", "sizeOrientationSwapPossible": true, "confidence": "candidate", "sourceHandles": node["sources"]}));
                    reviews.push(json!({"type": "vertical_duct_connection_review", "id": format!("vertical-{id}"), "message": "Check that this dashed obtuse centerline pair connects to a duct corner; width and height may swap across this riser."}));
                }
            }
            continue;
        }
        if !pipe_set.contains(&layer.to_lowercase()) {
            continue;
        }
        let kind = match degree {
            1 => "endpoint",
            2 => {
                if (180.0 - node_angle(node, &source_steps).unwrap_or(180.0)).abs() <= 12.0 {
                    "straight"
                } else {
                    "elbow"
                }
            }
            3 => "tee",
            4 => "cross",
            _ => "junction_review",
        };
        let id = node["id"].as_str().unwrap_or("node");
        junction_candidates.push(json!({"id": format!("junction-{id}"), "type": kind, "nodeId": id, "position": node["position"], "degree": degree, "connectedStepIds": node["steps"], "sourceHandles": node["sources"], "confidence": if degree <= 4 { "topology" } else { "review" }}));
        if degree > 4 {
            reviews.push(json!({"type": "unsupported_junction_degree", "componentId": format!("duct-{id}"), "degree": degree}));
        }
    }
    let connections: Vec<Value> = base["steps"].as_array().into_iter().flatten().filter(|step| pipe_set.contains(&step["layer"].as_str().unwrap_or_default().to_lowercase())).map(|step| json!({"stepId": step["id"], "from": step["startNode"], "to": step["endNode"], "sourceHandles": step["sources"], "layer": step["layer"]})).collect();

    let mut pipe_base = base.clone();
    pipe_base["steps"] =
        json!(base["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|step| pipe_set
                .contains(&step["layer"].as_str().unwrap_or_default().to_lowercase()))
            .collect::<Vec<_>>());
    pipe_base["nodes"] =
        json!(base["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|node| pipe_set
                .contains(&node["layer"].as_str().unwrap_or_default().to_lowercase()))
            .collect::<Vec<_>>());
    let all_faces = extract_faces(&pipe_base);
    let step_map: HashMap<&str, &Value> = base["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s["id"].as_str()?, s)))
        .collect();
    let mut faces: Vec<Face> = all_faces
        .into_iter()
        .filter(|face| {
            face.steps.iter().any(|id| {
                step_map.get(id.as_str()).is_some_and(|s| {
                    pipe_set.contains(&s["layer"].as_str().unwrap_or_default().to_lowercase())
                })
            })
        })
        .collect();
    faces.sort_by(|a, b| {
        a.center[0].total_cmp(&b.center[0]).then(
            a.center[1]
                .total_cmp(&b.center[1])
                .then(a.area.total_cmp(&b.area)),
        )
    });
    let mut dimension_texts = Vec::new();
    for text in base["texts"].as_array().into_iter().flatten() {
        if !annotation_set.is_empty()
            && !annotation_set.contains(&text["layer"].as_str().unwrap_or_default().to_lowercase())
        {
            continue;
        }
        if let (Some(source), Some(position)) =
            (text["source"].as_str(), text["position"].as_array())
        {
            if let (Some(x), Some(y)) = (
                position.first().and_then(Value::as_f64),
                position.get(1).and_then(Value::as_f64),
            ) {
                let size = text["text"].as_str().and_then(parse_duct_size);
                let measurement = text["measurement"].as_f64();
                let is_dimension = text["entityType"] == "DIMENSION";
                if size.is_some() || measurement.is_some() || is_dimension {
                    dimension_texts.push(json!({
                        "source":source,
                        "text":text["text"],
                        "position":[x,y],
                        "size":size.map(|value| json!([value[0],value[1]])).unwrap_or(Value::Null),
                        "measurement":measurement,
                        "entityType":text["entityType"],
                        "layer":text["layer"]
                    }));
                }
            }
        }
    }
    let mut dimension_matches: Vec<Option<(usize, f64)>> = vec![None; faces.len()];
    for (text_index, text) in dimension_texts.iter().enumerate() {
        if text["size"].as_array().is_none() {
            continue;
        }
        let point = [
            text["position"][0].as_f64().unwrap_or_default(),
            text["position"][1].as_f64().unwrap_or_default(),
        ];
        let nearest = faces
            .iter()
            .enumerate()
            .map(|(face_index, face)| (face_index, distance_to_face(point, face)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((face_index, distance)) = nearest.filter(|(_, distance)| *distance <= 500.0) {
            if dimension_matches[face_index].is_none_or(|(_, previous)| distance < previous) {
                dimension_matches[face_index] = Some((text_index, distance));
            }
        }
    }
    let mut component_records: Vec<Value> = faces.iter().enumerate().map(|(i,face)| {
        let mut lengths=Vec::new();
        for pair in face.points.windows(2).chain(std::iter::once([face.points[face.points.len()-1],face.points[0]].as_slice())) { let d=(pair[0][0]-pair[1][0]).hypot(pair[0][1]-pair[1][1]); if d>options.tolerance*0.05 { lengths.push(d); } }
        let dimension_match=dimension_matches[i];
        let size=dimension_match.map(|(ti,_)|dimension_texts[ti]["size"].clone());
        let width=size.as_ref().and_then(|x|x[0].as_f64()); let height=size.as_ref().and_then(|x|x[1].as_f64());
        let mut ports=Vec::new();
        if lengths.len()==4 {
            let min=lengths.iter().copied().fold(f64::INFINITY,f64::min); let max=lengths.iter().copied().fold(0.0,f64::max);
            if (max-min).abs()>options.tolerance { for (i,l) in lengths.iter().enumerate() { if (*l-min).abs()<=options.tolerance { ports.push(json!({"edgeIndex":i,"width":l})); } } }
            else { ports.extend(lengths.iter().enumerate().map(|(i,l)|json!({"edgeIndex":i,"width":l}))); }
        }
        let kind=if lengths.len()==4 {"duct_segment"} else if lengths.len()>4 {"duct_fitting_candidate"} else {"closed_duct_outline"};
        json!({"id":format!("duct-face-{i}"),"type":kind,"classificationEvidence":"closed_outline_and_parallel_edges","area":face.area,"centroid":face.center,"boundaryNodes":face.nodes,"boundaryStepIds":face.steps,"ports":ports,"size":size,"sizeSource":dimension_match.map(|(ti,_)|dimension_texts[ti]["source"].clone()),"sizeEvidence":if dimension_match.is_some(){"dimension_text"}else{"missing"},"width":width,"height":height,"sizeOrientationSwapPossible":false,"confidence":if size.is_some(){"outline_and_dimension_text"}else{"outline_only"},"sourceHandles":face.steps.iter().flat_map(|id|step_map.get(id.as_str()).into_iter().flat_map(|s|s["sources"].as_array().into_iter().flatten().filter_map(Value::as_str))).collect::<HashSet<_>>()})
    }).collect();
    let face_steps: HashSet<String> = faces
        .iter()
        .flat_map(|face| face.steps.iter().cloned())
        .collect();
    let pipe_steps: Vec<&Value> = base["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|step| {
            pipe_set.contains(&step["layer"].as_str().unwrap_or_default().to_lowercase())
                && !face_steps.contains(step["id"].as_str().unwrap_or_default())
        })
        .collect();
    let mut used_dimension_texts: HashSet<usize> = dimension_matches
        .iter()
        .filter_map(|m| m.map(|x| x.0))
        .collect();
    let mut paired_edge_candidates = Vec::new();
    let mut claimed_steps = HashSet::new();
    for i in 0..pipe_steps.len() {
        for j in i + 1..pipe_steps.len() {
            let (a, b) = (pipe_steps[i], pipe_steps[j]);
            let aid = a["id"].as_str().unwrap_or_default();
            let bid = b["id"].as_str().unwrap_or_default();
            let p = [
                a["start"][0].as_f64().unwrap_or_default(),
                a["start"][1].as_f64().unwrap_or_default(),
            ];
            let q = [
                a["end"][0].as_f64().unwrap_or_default(),
                a["end"][1].as_f64().unwrap_or_default(),
            ];
            let r = [
                b["start"][0].as_f64().unwrap_or_default(),
                b["start"][1].as_f64().unwrap_or_default(),
            ];
            let s = [
                b["end"][0].as_f64().unwrap_or_default(),
                b["end"][1].as_f64().unwrap_or_default(),
            ];
            let u = [q[0] - p[0], q[1] - p[1]];
            let v = [s[0] - r[0], s[1] - r[1]];
            let lu = u[0].hypot(u[1]);
            let lv = v[0].hypot(v[1]);
            if lu < 1e-8 || lv < 1e-8 {
                continue;
            }
            let cross = (u[0] * v[1] - u[1] * v[0]).abs() / (lu * lv);
            if cross > 15f64.to_radians().sin() {
                continue;
            }
            let signed_offset = (r[0] - p[0]) * (-u[1] / lu) + (r[1] - p[1]) * (u[0] / lu);
            let offset = signed_offset.abs();
            let dir = [u[0] / lu, u[1] / lu];
            let project = |x: [f64; 2]| (x[0] - p[0]) * dir[0] + (x[1] - p[1]) * dir[1];
            let lo = 0.0f64.max(project(r).min(project(s)));
            let hi = lu.min(project(r).max(project(s)));
            if hi - lo <= options.tolerance {
                continue;
            }
            let midpoint = [
                (p[0] + q[0] + r[0] + s[0]) * 0.25,
                (p[1] + q[1] + r[1] + s[1]) * 0.25,
            ];
            let best = dimension_texts
                .iter()
                .enumerate()
                .filter(|(ti, _)| !used_dimension_texts.contains(ti))
                .map(|(ti, t)| {
                    let d = (t["position"][0].as_f64().unwrap_or_default() - midpoint[0])
                        .hypot(t["position"][1].as_f64().unwrap_or_default() - midpoint[1]);
                    (ti, d)
                })
                .min_by(|x, y| x.1.total_cmp(&y.1));
            let Some((text_index, text_distance)) = best.filter(|(_, distance)| *distance < 500.0)
            else {
                continue;
            };
            if dimension_texts[text_index]["size"].as_array().is_none() {
                continue;
            }
            let size = &dimension_texts[text_index]["size"];
            let w = size[0].as_f64().unwrap_or_default();
            let h = size[1].as_f64().unwrap_or_default();
            let size_tol = options.tolerance.max(2.0);
            if (offset - w).abs() > size_tol && (offset - h).abs() > size_tol {
                continue;
            }
            let low0 = [p[0] + dir[0] * lo, p[1] + dir[1] * lo];
            let low1 = [
                low0[0] - dir[1] * signed_offset,
                low0[1] + dir[0] * signed_offset,
            ];
            let high0 = [p[0] + dir[0] * hi, p[1] + dir[1] * hi];
            let high1 = [
                high0[0] - dir[1] * signed_offset,
                high0[1] + dir[0] * signed_offset,
            ];
            if claimed_steps.contains(aid) || claimed_steps.contains(bid) {
                continue;
            }
            claimed_steps.insert(aid.to_string());
            claimed_steps.insert(bid.to_string());
            used_dimension_texts.insert(text_index);
            let id = format!("duct-pair-{}-{}", aid, bid);
            let candidate = json!({"id":id,"type":"duct_segment","classificationEvidence":"dimension_matched_parallel_edge_pair","edgeStepIds":[aid,bid],"size":size,"sizeSource":dimension_texts[text_index]["source"],"sizeEvidence":"parallel_spacing_and_dimension_text","overlapLength":hi-lo,"edgeSpacing":offset,"syntheticPorts":[{"from":low0,"to":low1,"source":"geometric_port_completion"},{"from":high0,"to":high1,"source":"geometric_port_completion"}],"confidence":"paired_edges_and_dimension","sourceHandles":a["sources"].as_array().into_iter().flatten().chain(b["sources"].as_array().into_iter().flatten()).cloned().collect::<HashSet<_>>(),"dimensionMatchDistance":text_distance});
            paired_edge_candidates.push(candidate.clone());
            component_records.push(candidate);
        }
    }
    let mut face_connections = Vec::new();
    for i in 0..faces.len() {
        for j in i + 1..faces.len() {
            let shared: Vec<_> = faces[i]
                .steps
                .iter()
                .filter(|id| faces[j].steps.contains(id))
                .cloned()
                .collect();
            if !shared.is_empty() {
                face_connections.push(json!({"from":component_records[i]["id"],"to":component_records[j]["id"],"sharedBoundaryStepIds":shared,"connectionType":"shared_port_edge"}));
            }
        }
    }
    let centroid_by_component: HashMap<String, [f64; 2]> = component_records
        .iter()
        .filter_map(|component| {
            Some((
                component["id"].as_str()?.to_owned(),
                [
                    component["centroid"][0].as_f64()?,
                    component["centroid"][1].as_f64()?,
                ],
            ))
        })
        .collect();
    for component in &mut component_records {
        let id = component["id"].clone();
        let ports: HashSet<String> = face_connections
            .iter()
            .filter(|connection| connection["from"] == id || connection["to"] == id)
            .flat_map(|connection| {
                connection["sharedBoundaryStepIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        let count = ports.len();
        if count >= 3 {
            component["type"] = json!(match count {
                3 => "tee",
                4 => "cross",
                _ => "multiport_fitting_candidate",
            });
            component["portsConnected"] = json!(count);
            component["confidence"] = json!("closed_outline_and_shared_port_edges");
        } else if count == 2 {
            let center = [
                component["centroid"][0].as_f64().unwrap_or_default(),
                component["centroid"][1].as_f64().unwrap_or_default(),
            ];
            let directions: Vec<[f64; 2]> = face_connections
                .iter()
                .filter(|connection| connection["from"] == id || connection["to"] == id)
                .filter_map(|connection| {
                    let other_id = if connection["from"] == id {
                        connection["to"].as_str()?
                    } else {
                        connection["from"].as_str()?
                    };
                    let other = centroid_by_component.get(other_id)?;
                    let x = other[0] - center[0];
                    let y = other[1] - center[1];
                    let n = x.hypot(y);
                    (n > 1e-9).then_some([x / n, y / n])
                })
                .collect();
            if directions.len() == 2 {
                component["type"] = json!(if directions[0][0] * directions[1][0]
                    + directions[0][1] * directions[1][1]
                    < -0.9781476
                {
                    "straight"
                } else {
                    "elbow"
                });
                component["portsConnected"] = json!(2);
            }
        }
    }
    let mut size_by_component: HashMap<String, Value> = component_records
        .iter()
        .filter_map(|c| Some((c["id"].as_str()?.into(), c["size"].clone())))
        .filter(|(_, s)| !s.is_null())
        .collect();
    let mut reducer_candidates = Vec::new();
    let component_ids: Vec<String> = component_records
        .iter()
        .filter_map(|component| component["id"].as_str().map(str::to_owned))
        .collect();
    for _ in 0..component_records.len().min(100) {
        let mut changed = false;
        for id in &component_ids {
            let Some(component) = component_records
                .iter()
                .find(|component| component["id"] == *id)
            else {
                continue;
            };
            if !matches!(
                component["type"].as_str(),
                Some("duct_segment" | "straight" | "elbow")
            ) || size_by_component.contains_key(id)
            {
                continue;
            }
            let neighbors: Vec<Value> = face_connections
                .iter()
                .filter_map(|conn| {
                    let other = if conn["from"] == *id {
                        conn["to"].as_str()?
                    } else if conn["to"] == *id {
                        conn["from"].as_str()?
                    } else {
                        return None;
                    };
                    size_by_component.get(other).cloned()
                })
                .collect();
            let distinct: HashSet<String> = neighbors
                .iter()
                .filter_map(|v| serde_json::to_string(v).ok())
                .collect();
            if distinct.len() > 1 {
                if let Some(c) = component_records.iter_mut().find(|c| c["id"] == *id) {
                    c["type"] = json!("reducer_candidate");
                    c["confidence"] = json!("connected_size_transition");
                    c["sizeOrientationSwapPossible"] = json!(true);
                    c["adjacentSizes"] = json!(neighbors);
                }
                reducer_candidates.push(json!({"componentId":id,"type":"reducer_candidate","adjacentSizes":neighbors,"widthHeightSwapPossible":true,"confidence":"topology_and_size_transition"}));
            } else if let Some(size) = neighbors.first() {
                size_by_component.insert(id.clone(), size.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for component in &mut component_records {
        if component["size"].is_null() {
            if let Some(size) = size_by_component.get(component["id"].as_str().unwrap_or_default())
            {
                component["size"] = size.clone();
                component["sizeEvidence"] = json!("propagated_over_component_connections");
            }
        }
    }
    let mut applied = Vec::new();
    for correction in options.corrections.as_array().into_iter().flatten() {
        let (Some(id), Some(kind)) = (
            correction["componentId"].as_str(),
            correction["type"].as_str(),
        ) else {
            continue;
        };
        if let Some(component) = component_records
            .iter_mut()
            .find(|component| component["id"] == id)
        {
            component["type"] = json!(kind);
            component["confidence"] = json!("user_corrected");
            applied.push(id.to_string());
        }
    }
    let highlight_handles = entity_handles_for_layers(doc, &layers);
    json!({"ok": true, "status": if options.finalize { "finalized" } else { "review_required" }, "schemaVersion": "ocs-duct-recognition/2", "engine": "native-duct-outline", "confirmedLayers": {"pipe": options.pipe_layers, "centerline": options.centerline_layers, "annotation": options.annotation_layers}, "highlightHandles": highlight_handles, "highlightHandlesTruncated": highlight_handles.len() >= MAX_HIGHLIGHT_HANDLES, "components": component_records, "pairedEdgeCandidates": paired_edge_candidates, "junctionCandidates": junction_candidates, "connections": face_connections, "centerlineConnections": connections, "annotationTexts": base["texts"], "dimensionTexts": dimension_texts, "reducerCandidates": reducer_candidates, "verticalDuctCandidates": vertical_evidence, "reviewItems": reviews, "correctionsApplied": applied, "complete": base["complete"], "coverage": base["coverage"], "next": "Review closed-outline components, paired-edge ports, dimension matches, propagated sizes, reducer candidates and riser evidence. Results remain provisional until reviewed."})
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::tables::Layer;
    use codec::{entities::Line, types::Vector3, EntityType};

    fn add_line(doc: &mut CadDocument, layer: &str, a: [f64; 2], b: [f64; 2]) {
        let mut line =
            Line::from_points(Vector3::new(a[0], a[1], 0.0), Vector3::new(b[0], b[1], 0.0));
        line.common.layer = layer.into();
        doc.add_entity(EntityType::Line(line)).unwrap();
    }

    #[test]
    fn size_parser_handles_ship_duct_dimension_formats() {
        assert_eq!(
            parse_duct_size("HF/PY 1250×630 (4.30)"),
            Some([1250.0, 630.0])
        );
        assert_eq!(parse_duct_size("800*500"), Some([800.0, 500.0]));
        assert_eq!(parse_duct_size("DN150"), None);
    }

    #[test]
    fn closed_rectangle_is_extracted_with_long_edges_and_ports() {
        let mut base = json!({
            "nodes":[
                {"id":"a","position":[0.0,0.0]}, {"id":"b","position":[100.0,0.0]},
                {"id":"c","position":[100.0,50.0]}, {"id":"d","position":[0.0,50.0]}
            ],
            "steps":[
                {"id":"ab","startNode":"a","endNode":"b","layer":"PIPES"},
                {"id":"bc","startNode":"b","endNode":"c","layer":"PIPES"},
                {"id":"cd","startNode":"c","endNode":"d","layer":"PIPES"},
                {"id":"da","startNode":"d","endNode":"a","layer":"PIPES"}
            ]
        });
        base["texts"] = json!([]);
        let faces = extract_faces(&base);
        assert_eq!(faces.len(), 1);
        assert!((faces[0].area - 5000.0).abs() < 1e-9);
        let lengths: Vec<f64> = faces[0]
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = faces[0].points[(i + 1) % faces[0].points.len()];
                (p[0] - q[0]).hypot(p[1] - q[1])
            })
            .collect();
        assert_eq!(lengths.iter().filter(|length| **length == 50.0).count(), 2);
    }

    #[test]
    fn recognition_matches_dimensions_to_closed_duct_outlines() {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("PIPES")).unwrap();
        doc.layers.add(Layer::new("DIMS")).unwrap();
        add_line(&mut doc, "PIPES", [0.0, 0.0], [100.0, 0.0]);
        add_line(&mut doc, "PIPES", [100.0, 0.0], [100.0, 50.0]);
        add_line(&mut doc, "PIPES", [100.0, 50.0], [0.0, 50.0]);
        add_line(&mut doc, "PIPES", [0.0, 50.0], [0.0, 0.0]);
        let mut label = codec::entities::Text::new();
        label.value = "800x500".into();
        label.insertion_point = Vector3::new(50.0, 25.0, 0.0);
        label.common.layer = "DIMS".into();
        doc.add_entity(EntityType::Text(label)).unwrap();
        let result = recognize_duct(
            &doc,
            &DuctOptions {
                pipe_layers: vec!["PIPES".into()],
                annotation_layers: vec!["DIMS".into()],
                tolerance: 0.01,
                ..Default::default()
            },
        );
        assert_eq!(result["components"].as_array().unwrap().len(), 1);
        assert_eq!(result["components"][0]["type"], "duct_segment");
        assert_eq!(result["components"][0]["size"], json!([800.0, 500.0]));
        assert_eq!(result["components"][0]["sizeEvidence"], "dimension_text");
        assert_eq!(
            result["components"][0]["ports"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn recognition_reads_dimension_and_multileader_annotation_entities() {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("PIPES")).unwrap();
        doc.layers.add(Layer::new("DIMS")).unwrap();

        let mut dimension = codec::entities::DimensionLinear::new(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(100.0, 0.0, 0.0),
        );
        dimension.base.text_middle_point = Vector3::new(50.0, -20.0, 0.0);
        dimension.base.set_text_override(Some("800x500".into()));
        dimension.base.common.layer = "DIMS".into();
        doc.add_entity(EntityType::Dimension(Box::new(
            codec::entities::Dimension::Linear(dimension),
        )))
        .unwrap();

        let mut leader = codec::entities::MultiLeader::with_text(
            "600x400",
            Vector3::new(200.0, -20.0, 0.0),
            vec![Vector3::new(200.0, 0.0, 0.0)],
        );
        leader.common.layer = "DIMS".into();
        doc.add_entity(EntityType::MultiLeader(Box::new(leader)))
            .unwrap();

        let result = recognize_duct(
            &doc,
            &DuctOptions {
                pipe_layers: vec!["PIPES".into()],
                annotation_layers: vec!["DIMS".into()],
                tolerance: 0.01,
                ..Default::default()
            },
        );
        let annotations = result["annotationTexts"].as_array().unwrap();
        assert!(annotations
            .iter()
            .any(|text| text["entityType"] == "DIMENSION"));
        assert!(annotations
            .iter()
            .any(|text| text["entityType"] == "MULTILEADER" && text["text"] == "600x400"));
        assert!(result["dimensionTexts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|text| text["size"] == json!([800.0, 500.0])));
    }

    #[test]
    fn parallel_edges_without_endcaps_get_dimension_matched_synthetic_ports() {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("PIPES")).unwrap();
        doc.layers.add(Layer::new("DIMS")).unwrap();
        add_line(&mut doc, "PIPES", [0.0, 0.0], [100.0, 0.0]);
        add_line(&mut doc, "PIPES", [0.0, 500.0], [100.0, 500.0]);
        let mut label = codec::entities::Text::new();
        label.value = "800x500".into();
        label.insertion_point = Vector3::new(50.0, 250.0, 0.0);
        label.common.layer = "DIMS".into();
        doc.add_entity(EntityType::Text(label)).unwrap();
        let result = recognize_duct(
            &doc,
            &DuctOptions {
                pipe_layers: vec!["PIPES".into()],
                annotation_layers: vec!["DIMS".into()],
                tolerance: 0.01,
                ..Default::default()
            },
        );
        assert_eq!(result["pairedEdgeCandidates"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["pairedEdgeCandidates"][0]["syntheticPorts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            result["pairedEdgeCandidates"][0]["size"],
            json!([800.0, 500.0])
        );
        assert_eq!(result["pairedEdgeCandidates"][0]["overlapLength"], 100.0);
    }

    fn adjacent_rectangles(sizes: &[Option<&str>]) -> CadDocument {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("PIPES")).unwrap();
        doc.layers.add(Layer::new("DIMS")).unwrap();
        for i in 0..sizes.len() {
            let x0 = i as f64 * 100.0;
            let x1 = x0 + 100.0;
            add_line(&mut doc, "PIPES", [x0, 0.0], [x1, 0.0]);
            add_line(&mut doc, "PIPES", [x1, 0.0], [x1, 50.0]);
            add_line(&mut doc, "PIPES", [x1, 50.0], [x0, 50.0]);
            add_line(&mut doc, "PIPES", [x0, 50.0], [x0, 0.0]);
            if let Some(size) = sizes[i] {
                let mut label = codec::entities::Text::new();
                label.value = size.into();
                label.insertion_point = Vector3::new(x0 + 20.0, 3.0, 0.0);
                label.common.layer = "DIMS".into();
                doc.add_entity(EntityType::Text(label)).unwrap();
            }
        }
        doc
    }

    fn adjacent_rectangles_options() -> DuctOptions {
        DuctOptions {
            pipe_layers: vec!["PIPES".into()],
            annotation_layers: vec!["DIMS".into()],
            tolerance: 0.01,
            ..Default::default()
        }
    }

    fn add_rect(doc: &mut CadDocument, x0: f64, y0: f64, x1: f64, y1: f64) {
        add_line(doc, "PIPES", [x0, y0], [x1, y0]);
        add_line(doc, "PIPES", [x1, y0], [x1, y1]);
        add_line(doc, "PIPES", [x1, y1], [x0, y1]);
        add_line(doc, "PIPES", [x0, y1], [x0, y0]);
    }

    #[test]
    fn three_connected_closed_outlines_are_classified_as_tee() {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("PIPES")).unwrap();
        doc.layers.add(Layer::new("DIMS")).unwrap();
        add_rect(&mut doc, 0.0, 0.0, 100.0, 50.0);
        add_rect(&mut doc, -50.0, 0.0, 0.0, 50.0);
        add_rect(&mut doc, 100.0, 0.0, 150.0, 50.0);
        add_rect(&mut doc, 25.0, 50.0, 75.0, 100.0);

        let result = recognize_duct(&doc, &adjacent_rectangles_options());
        let tee = result["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|component| component["portsConnected"] == 3)
            .unwrap_or_else(|| {
                panic!("central outline should have three connected ports: {result}")
            });
        assert_eq!(tee["type"], "tee");
    }

    #[test]
    fn connected_unmarked_straight_duct_inherits_dimension() {
        let doc = adjacent_rectangles(&[Some("800x500"), None]);
        let result = recognize_duct(&doc, &adjacent_rectangles_options());
        let components = result["components"].as_array().unwrap();
        let inherited = components
            .iter()
            .find(|component| component["sizeEvidence"] == "propagated_over_component_connections")
            .unwrap_or_else(|| panic!("second duct inherits the connected dimension: {result}"));
        assert_eq!(inherited["size"], json!([800.0, 500.0]));
        assert_eq!(result["connections"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn unmarked_straight_between_different_sizes_is_reducer_candidate() {
        let doc = adjacent_rectangles(&[Some("800x500"), None, Some("600x400")]);
        let result = recognize_duct(&doc, &adjacent_rectangles_options());
        assert_eq!(
            result["reducerCandidates"].as_array().unwrap().len(),
            1,
            "{result}"
        );
        let reducer_id = result["reducerCandidates"][0]["componentId"].clone();
        let reducer = result["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|component| component["id"] == reducer_id)
            .expect("transition component");
        assert_eq!(reducer["type"], "reducer_candidate");
        assert_eq!(reducer["sizeOrientationSwapPossible"], true);
    }

    #[test]
    fn layer_roots_and_category_priority_match_ship_classifier() {
        let mut doc = CadDocument::new();
        for name in [
            "05-\u{9001}\u{98ce}\u{7ba1}",
            "05-\u{9001}\u{98ce}\u{7ba1}-\u{4e2d}\u{5fc3}\u{7ebf}",
            "05-\u{9001}\u{98ce}\u{7ba1}-\u{6807}\u{6ce8}",
            "05-\u{9001}\u{98ce}-\u{7acb}\u{7ba1}",
        ] {
            doc.layers.add(Layer::new(name)).unwrap();
        }
        let groups = classify_duct_layers(&doc);
        let systems = groups["systems"].as_array().unwrap();
        assert!(systems.len() >= 1);
        let group = systems
            .iter()
            .find(|group| group["systemName"] == "05-\u{9001}\u{98ce}")
            .expect("ship duct system");
        assert_eq!(group["systemName"], "05-\u{9001}\u{98ce}");
        assert_eq!(group["pipeLayers"], json!(["05-\u{9001}\u{98ce}\u{7ba1}"]));
        assert_eq!(
            group["centerlineLayers"],
            json!(["05-\u{9001}\u{98ce}\u{7ba1}-\u{4e2d}\u{5fc3}\u{7ebf}"])
        );
        assert_eq!(
            group["annotationLayers"],
            json!(["05-\u{9001}\u{98ce}\u{7ba1}-\u{6807}\u{6ce8}"])
        );
        assert_eq!(
            group["verticalPipeLayers"],
            json!(["05-\u{9001}\u{98ce}-\u{7acb}\u{7ba1}"])
        );
    }
}
