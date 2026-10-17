use super::{Segment, Style};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

const MAX_STEPS: usize = 8_000;
pub(super) fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}
fn project(p: [f64; 2], s: &Segment) -> (f64, f64) {
    let dx = s.b[0] - s.a[0];
    let dy = s.b[1] - s.a[1];
    let t = ((p[0] - s.a[0]) * dx + (p[1] - s.a[1]) * dy) / (dx * dx + dy * dy);
    (t, dist(p, lerp(s.a, s.b, t)))
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn bounds_overlap(a: &Segment, b: &Segment, tol: f64) -> bool {
    (0..2).all(|i| {
        a.a[i].min(a.b[i]) - tol <= b.a[i].max(b.b[i])
            && b.a[i].min(b.b[i]) - tol <= a.a[i].max(a.b[i])
    })
}
struct Node {
    p: [f64; 2],
    style: Style,
    steps: Vec<usize>,
}
fn node(
    nodes: &mut Vec<Node>,
    cells: &mut HashMap<(i64, i64), Vec<usize>>,
    p: [f64; 2],
    style: &Style,
    tol: f64,
) -> usize {
    let key = ((p[0] / tol).floor() as i64, (p[1] / tol).floor() as i64);
    for x in key.0.saturating_sub(1)..=key.0.saturating_add(1) {
        for y in key.1.saturating_sub(1)..=key.1.saturating_add(1) {
            if let Some(ids) = cells.get(&(x, y)) {
                for &i in ids {
                    if nodes[i].style == *style && dist(nodes[i].p, p) <= tol {
                        return i;
                    }
                }
            }
        }
    }
    let id = nodes.len();
    nodes.push(Node {
        p,
        style: style.clone(),
        steps: vec![],
    });
    cells.entry(key).or_default().push(id);
    id
}

/// Split at collinear overlap endpoints and T contacts only. Interior X intersections remain review-only.
pub(super) fn build(segments: &[Segment], tol: f64) -> (Vec<Value>, Vec<Value>, Vec<Value>, bool) {
    let mut cuts: Vec<Vec<f64>> = segments.iter().map(|_| vec![0.0, 1.0]).collect();
    let mut reviews = Vec::new();
    let mut truncated = false;
    for (i, a) in segments.iter().enumerate() {
        for (j, b) in segments.iter().enumerate().skip(i + 1) {
            if a.style != b.style || !bounds_overlap(a, b, tol) {
                continue;
            }
            if a.bridge || b.bridge {
                continue;
            }
            for p in [a.a, a.b] {
                let (t, d) = project(p, b);
                if t > 0.0 && t < 1.0 && d <= tol {
                    cuts[j].push(t);
                }
            }
            for p in [b.a, b.b] {
                let (t, d) = project(p, a);
                if t > 0.0 && t < 1.0 && d <= tol {
                    cuts[i].push(t);
                }
            }
            let u = [a.b[0] - a.a[0], a.b[1] - a.a[1]];
            let v = [b.b[0] - b.a[0], b.b[1] - b.a[1]];
            let det = cross(u, v);
            if det.abs() < 1e-12 * dist(a.a, a.b) * dist(b.a, b.b) {
                continue;
            }
            let w = [b.a[0] - a.a[0], b.a[1] - a.a[1]];
            let ta = cross(w, v) / det;
            let tb = cross(w, u) / det;
            if ta > 0.0 && ta < 1.0 && tb > 0.0 && tb < 1.0 {
                let p = lerp(a.a, a.b, ta);
                if [a.a, a.b, b.a, b.b].iter().all(|&e| dist(e, p) > tol) {
                    if reviews.len() < super::MAX_REVIEW {
                        reviews.push(json!({"type":"unconnected_crossing","position":p,"sources":a.sources.iter().chain(&b.sources).collect::<Vec<_>>() }));
                    } else {
                        truncated = true;
                    }
                }
            }
        }
    }
    let mut nodes = Vec::new();
    let mut cells = HashMap::new();
    let mut edges: HashMap<(usize, usize, bool), usize> = HashMap::new();
    let mut pieces: Vec<(usize, usize, Segment)> = vec![];
    'outer: for (s, mut ts) in segments.iter().zip(cuts) {
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
        for t in ts.windows(2) {
            let a = lerp(s.a, s.b, t[0]);
            let b = lerp(s.a, s.b, t[1]);
            if dist(a, b) < 1e-9 {
                continue;
            }
            let na = node(&mut nodes, &mut cells, a, &s.style, tol);
            let nb = node(&mut nodes, &mut cells, b, &s.style, tol);
            if na == nb {
                continue;
            }
            let key = (na.min(nb), na.max(nb), s.bridge);
            if let Some(&idx) = edges.get(&key) {
                pieces[idx].2.sources.extend(s.sources.clone());
                continue;
            }
            if pieces.len() >= MAX_STEPS {
                truncated = true;
                break 'outer;
            }
            let id = pieces.len();
            nodes[na].steps.push(id);
            nodes[nb].steps.push(id);
            edges.insert(key, id);
            pieces.push((na, nb, Segment { a, b, ..s.clone() }));
        }
    }
    let steps=pieces.iter_mut().enumerate().map(|(i,(a,b,s))|{s.sources.sort();s.sources.dedup();json!({"id":format!("s{i}"),"start":[s.a[0],s.a[1],0.0],"end":[s.b[0],s.b[1],0.0],"startNode":format!("n{a}"),"endNode":format!("n{b}"),"sources":s.sources,"layer":s.style.layer,"linetype":s.style.linetype,"color":s.style.color,"bridge":s.bridge,"geometry":if s.bridge{"arc_endpoints"}else{"line"}})}).collect();
    let nodes=nodes.iter().enumerate().filter(|(_,n)|!n.steps.is_empty()).map(|(i,n)|{let mut sources:Vec<_>=n.steps.iter().flat_map(|&s|pieces[s].2.sources.iter().cloned()).collect();sources.sort();sources.dedup();let degree=n.steps.iter().collect::<HashSet<_>>().len();json!({"id":format!("n{i}"),"position":n.p,"layer":n.style.layer,"degree":degree,"kind":if degree>2{"junction"}else if degree==2{"continuation"}else{"endpoint"},"steps":n.steps.iter().map(|s|format!("s{s}")).collect::<Vec<_>>(),"sources":sources})}).collect();
    (steps, nodes, reviews, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(a: [f64; 2], b: [f64; 2], id: &str) -> Segment {
        Segment {
            a,
            b,
            sources: vec![id.into()],
            style: Style {
                layer: "PIPES".into(),
                color: "7".into(),
                linetype: "CONTINUOUS".into(),
            },
            bridge: false,
        }
    }
    #[test]
    fn t_and_x_are_distinct() {
        let (st, n, _, _) = build(
            &[s([0., 0.], [10., 0.], "A"), s([5., 0.], [5., 5.], "B")],
            0.01,
        );
        assert_eq!(st.len(), 3);
        assert!(n.iter().any(|x| x["degree"] == 3));
        let (st, n, r, _) = build(
            &[s([0., 0.], [10., 0.], "A"), s([5., -5.], [5., 5.], "B")],
            0.01,
        );
        assert_eq!(st.len(), 2);
        assert!(n.iter().all(|x| x["degree"] == 1));
        assert_eq!(r[0]["type"], "unconnected_crossing");
    }
    #[test]
    fn overlap_retains_sources() {
        let (st, _, _, _) = build(
            &[s([0., 0.], [10., 0.], "A"), s([5., 0.], [15., 0.], "B")],
            0.01,
        );
        assert_eq!(st.len(), 3);
        assert!(st.iter().any(|e| e["sources"] == json!(["A", "B"])));
    }
    #[test]
    fn different_styles_and_bridge_chords_remain_separate() {
        let a = s([0., 0.], [10., 0.], "A");
        let mut b = s([5., 0.], [5., 5.], "B");
        b.style.color = "1".into();
        let (st, n, _, _) = build(&[a.clone(), b.clone()], 0.01);
        assert_eq!(st.len(), 2);
        assert!(n.iter().all(|e| e["degree"] == 1));
        b.style = a.style.clone();
        b.bridge = true;
        let (st, _, _, _) = build(&[a, b], 0.01);
        assert_eq!(st.len(), 2);
    }
    #[test]
    fn tolerance_searches_adjacent_cells() {
        let (_, n, _, _) = build(
            &[s([0., 0.], [9.99, 0.], "A"), s([10.01, 0.], [20., 0.], "B")],
            0.1,
        );
        assert_eq!(n.iter().filter(|e| e["degree"] == 2).count(), 1);
    }
}
