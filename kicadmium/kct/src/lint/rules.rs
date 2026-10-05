use crate::lint::{model::*, Config, Emitter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub family: String,
    pub stage: String,
    pub status: String,
    pub severity: String,
    pub confidence: String,
    pub detection: String,
    pub action: String,
    pub caveat: String,
}
pub fn catalog_cached() -> &'static Vec<Rule> {
    static RULES: std::sync::OnceLock<Vec<Rule>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        serde_json::from_str(include_str!("catalog.json")).expect("valid embedded catalog")
    })
}
pub fn catalog() -> Vec<Rule> {
    catalog_cached().clone()
}
pub fn has_contract(id: &str, c: &Config) -> bool {
    match id {
        "contract.pin_net" => !c.pin_nets.is_empty(),
        "contract.proximity" | "contract.side" => !c.proximity.is_empty(),
        "contract.alignment" => !c.groups.is_empty(),
        _ => !c.net_rules.is_empty(),
    }
}
fn near(a: Point, b: Point, c: &Config) -> bool {
    a.distance(b) <= c.join_tolerance_mm
}
fn touches(t: &Track, p: Point, layer: &str) -> bool {
    t.layer == layer && line_distance(p, t.a, t.b) <= t.width / 2. + 1e-6
}
fn attachments(e: &Emitter, t: usize, p: Point) -> usize {
    let b = e.board;
    let tr = &b.tracks[t];
    let tracks = b
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, x)| *i != t && x.net == tr.net && touches(x, p, &tr.layer))
        .count();
    let pads = b
        .pads
        .iter()
        .filter(|x| {
            x.net == tr.net && on_layer(&x.layers, &tr.layer) && pad_distance(p, x) <= tr.width / 2.
        })
        .count();
    let vias = b
        .vias
        .iter()
        .filter(|v| {
            v.net == tr.net
                && on_layer(&b.via_layers(v), &tr.layer)
                && v.at.distance(p) <= (v.size + tr.width) / 2.
        })
        .count();
    tracks + pads + vias
}
fn octile(a: Point, b: Point) -> f64 {
    let d = b.minus(a);
    let x = d.x.abs();
    let y = d.y.abs();
    x.max(y) + (2_f64.sqrt() - 1.) * x.min(y)
}
pub fn run(e: &mut Emitter) {
    let b = e.board;
    let c = e.config;
    for (i, t) in b.tracks.iter().enumerate() {
        let ids = || vec![t.id.clone()];
        let nets = || vec![t.net.clone()];
        let len = t.a.distance(t.b);
        if len <= c.join_tolerance_mm {
            e.emit(
                "trace.zero_length",
                ids(),
                nets(),
                t.a,
                "",
                format!("Trace length {len:.4} mm"),
                &[("length_mm", len)],
            );
        } else {
            if len < c.short_segment_mm {
                e.emit(
                    "trace.short_segment",
                    ids(),
                    nets(),
                    t.a,
                    "",
                    format!("Short segment ({len:.3} mm); may be a neck or tuning feature"),
                    &[("length_mm", len)],
                );
            }
            let angle = (t.b.y - t.a.y)
                .atan2(t.b.x - t.a.x)
                .to_degrees()
                .rem_euclid(45.);
            let dev = angle.min(45. - angle);
            if dev > c.angle_tolerance_deg {
                e.emit(
                    "trace.off_angle",
                    ids(),
                    nets(),
                    t.a,
                    "",
                    format!("{dev:.2}° from the nearest 45° direction"),
                    &[("deviation_deg", dev)],
                );
            }
        }
        if t.width < c.min_trace_mm {
            e.emit(
                "trace.minimum_width",
                ids(),
                nets(),
                t.a,
                "",
                format!("Width {:.3} mm below configured minimum", t.width),
                &[("width_mm", t.width), ("minimum_mm", c.min_trace_mm)],
            );
        }
        if t.net.is_empty() {
            e.emit(
                "trace.no_net",
                ids(),
                nets(),
                t.a,
                "",
                "Copper track has no net".into(),
                &[],
            );
        }
        // Stub hypotheses are not connectivity failures: zones and arcs may attach here.
        for p in [t.a, t.b] {
            if attachments(e, i, p) == 0 {
                let mut points = [t.a, t.b];
                points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
                e.emit("trace.open_end",ids(),nets(),p,if p==points[0]{"end0"}else{"end1"},"Endpoint has no modeled same-net copper attachment; inspect for a stub or unfinished route".into(),&[]);
            }
        }
        for p in &b.pads {
            if p.net == t.net && on_layer(&p.layers, &t.layer) {
                let (inside, out) = if pad_distance(t.a, p) == 0. {
                    (t.a, t.b)
                } else if pad_distance(t.b, p) == 0. {
                    (t.b, t.a)
                } else {
                    continue;
                };
                if pad_distance(out, p) == 0. || inside.distance(out) < c.short_segment_mm {
                    continue;
                }
                let v = out.minus(inside).rotate(-p.angle);
                let deg = v.y.atan2(v.x).to_degrees().rem_euclid(90.);
                let dev = deg.min(90. - deg);
                if dev > c.angle_tolerance_deg && p.shape != "circle" {
                    e.emit(
                        "pad.oblique_exit",
                        vec![t.id.clone(), p.id.clone()],
                        nets(),
                        inside,
                        "",
                        format!("Oblique exit from {}.{}", p.reference, p.number),
                        &[("deviation_deg", dev)],
                    );
                }
            }
        }
    }
    for i in 0..b.tracks.len() {
        let a = &b.tracks[i];
        for j in i + 1..b.tracks.len() {
            let t = &b.tracks[j];
            if a.net != t.net || a.layer != t.layer {
                continue;
            }
            let ids = || vec![a.id.clone(), t.id.clone()];
            let nets = || vec![a.net.clone()];
            if ((near(a.a, t.a, c) && near(a.b, t.b, c))
                || (near(a.a, t.b, c) && near(a.b, t.a, c)))
                && (a.width - t.width).abs() < 1e-6
            {
                e.emit(
                    "trace.duplicate",
                    ids(),
                    nets(),
                    a.a,
                    "",
                    "Duplicate straight copper segments".into(),
                    &[],
                );
                continue;
            }
            let mut joint = None;
            for (p, far) in [(a.a, a.b), (a.b, a.a)] {
                for (q, other) in [(t.a, t.b), (t.b, t.a)] {
                    if near(p, q, c) {
                        joint = Some((p, far, other));
                    }
                }
            }
            if let Some((p, far, other)) = joint {
                let u = far.minus(p);
                let v = other.minus(p);
                let lens = u.x.hypot(u.y) * v.x.hypot(v.y);
                if lens < 1e-12 {
                    continue;
                }
                let angle = ((u.x * v.x + u.y * v.y) / lens)
                    .clamp(-1., 1.)
                    .acos()
                    .to_degrees();
                if (180. - angle) < c.angle_tolerance_deg && (a.width - t.width).abs() < 1e-6 {
                    e.emit(
                        "trace.mergeable",
                        ids(),
                        nets(),
                        p,
                        "",
                        "Collinear same-width segments share an endpoint".into(),
                        &[],
                    );
                }
                if angle < 90. - c.angle_tolerance_deg {
                    e.emit(
                        "trace.acute_junction",
                        ids(),
                        nets(),
                        p,
                        "",
                        "Acute same-net junction or backtracking".into(),
                        &[("interior_deg", angle)],
                    );
                }
                if (a.width - t.width).abs() > 1e-6 {
                    e.emit(
                        "trace.width_transition",
                        ids(),
                        nets(),
                        p,
                        "",
                        "Trace width changes at a joint; confirm neck/clearance intent".into(),
                        &[
                            ("narrow_mm", a.width.min(t.width)),
                            ("wide_mm", a.width.max(t.width)),
                        ],
                    );
                }
            }
        }
    }
    for (i, v) in b.vias.iter().enumerate() {
        let ids = || vec![v.id.clone()];
        let nets = || vec![v.net.clone()];
        let ann = (v.size - v.drill) / 2.;
        if ann < c.min_annulus_mm {
            e.emit(
                "via.annulus",
                ids(),
                nets(),
                v.at,
                "",
                "Via annulus below configured minimum".into(),
                &[("annulus_mm", ann)],
            );
        }
        if v.drill < c.min_drill_mm {
            e.emit(
                "via.drill",
                ids(),
                nets(),
                v.at,
                "",
                "Via drill below configured minimum".into(),
                &[("drill_mm", v.drill)],
            );
        }
        if v.net.is_empty() {
            e.emit(
                "via.no_net",
                ids(),
                nets(),
                v.at,
                "",
                "Via has no net assignment".into(),
                &[],
            );
        }
        let vl = b.via_layers(v);
        for p in &b.pads {
            if vl.iter().any(|l| on_layer(&p.layers, l)) && pad_distance(v.at, p) < v.size / 2. {
                e.emit(
                    "via.pad_overlap",
                    vec![v.id.clone(), p.id.clone()],
                    vec![v.net.clone(), p.net.clone()],
                    v.at,
                    "",
                    format!(
                        "Via envelope overlaps {}.{}; verify via-in-pad process/clearance",
                        p.reference, p.number
                    ),
                    &[],
                );
            }
        }
        for w in &b.vias[i + 1..] {
            if v.net == w.net {
                let d = v.at.distance(w.at);
                if d < c.via_cluster_mm {
                    e.emit(
                        "via.cluster",
                        vec![v.id.clone(), w.id.clone()],
                        nets(),
                        v.at,
                        "",
                        "Nearby same-net vias; may be intentional thermal or stitching array"
                            .into(),
                        &[("distance_mm", d)],
                    );
                }
            }
        }
    }
    chains(e);
    contracts(e);
}
/// Endpoint-only graph for local topology heuristics. Branches stop a chain; no connectivity claims.
fn chains(e: &mut Emitter) {
    let b = e.board;
    let c = e.config;
    let n = b.tracks.len();
    let mut adjacency = vec![vec![]; n * 2];
    for i in 0..n {
        for j in i + 1..n {
            let a = &b.tracks[i];
            let z = &b.tracks[j];
            if a.net != z.net || a.layer != z.layer {
                continue;
            }
            for (k, p) in [a.a, a.b].iter().enumerate() {
                for (l, q) in [z.a, z.b].iter().enumerate() {
                    if near(*p, *q, c) {
                        adjacency[i * 2 + k].push(j * 2 + l);
                        adjacency[j * 2 + l].push(i * 2 + k);
                    }
                }
            }
        }
    }
    // Do not chain across pad/via landings or mid-segment T junctions.
    for i in 0..n {
        for (k, p) in [b.tracks[i].a, b.tracks[i].b].into_iter().enumerate() {
            if attachments(e, i, p) != adjacency[i * 2 + k].len() {
                adjacency[i * 2 + k].clear();
            }
        }
    }
    let mut used = BTreeSet::new();
    for start in 0..n * 2 {
        if adjacency[start].len() == 1 || used.contains(&(start / 2)) {
            continue;
        }
        let mut cur = start;
        let mut path = vec![];
        let mut points = vec![];
        loop {
            let i = cur / 2;
            if !used.insert(i) {
                break;
            }
            let t = &b.tracks[i];
            points.push(if cur % 2 == 0 { t.a } else { t.b });
            path.push(i);
            let other = cur ^ 1;
            if adjacency[other].len() != 1 {
                points.push(if other % 2 == 0 { t.a } else { t.b });
                break;
            }
            let next = adjacency[other][0];
            if adjacency[next].len() != 1 || used.contains(&(next / 2)) {
                points.push(if other % 2 == 0 { t.a } else { t.b });
                break;
            }
            cur = next;
        }
        if path.is_empty() || points.len() != path.len() + 1 {
            continue;
        }
        let first = points[0];
        let last = *points.last().unwrap();
        let ids = || path.iter().map(|&i| b.tracks[i].id.clone()).collect();
        let nets = || vec![b.tracks[path[0]].net.clone()];
        let len: f64 = path
            .iter()
            .map(|&i| b.tracks[i].a.distance(b.tracks[i].b))
            .sum();
        let lower = octile(first, last);
        if lower > c.join_tolerance_mm
            && len > lower * c.detour_ratio
            && len - lower > c.detour_excess_mm
        {
            e.emit("route.detour",ids(),nets(),first,"", "Chain takes a long path relative to unobstructed 45° lower bound; inspect obstacles and tuning".into(),&[("length_mm",len),("lower_bound_mm",lower),("ratio",len/lower)]);
        }
        let bends = points
            .windows(3)
            .filter(|w| {
                let a = w[1].minus(w[0]);
                let z = w[2].minus(w[1]);
                (a.x * z.y - a.y * z.x).abs() > 1e-6
            })
            .count();
        if bends >= c.bend_threshold {
            e.emit(
                "route.bend_count",
                ids(),
                nets(),
                first,
                "",
                "Many bends in an unbranched chain; inspect routing intent".into(),
                &[("bends", bends as f64)],
            );
        }
        if path.len() >= 3 {
            let mut pos = 1;
            while pos + 1 < path.len() {
                let width = b.tracks[path[pos]].width;
                let from = pos;
                while pos + 1 < path.len() && (b.tracks[path[pos + 1]].width - width).abs() < 1e-6 {
                    pos += 1;
                }
                if pos + 1 < path.len()
                    && b.tracks[path[from - 1]].width > width + 1e-6
                    && b.tracks[path[pos + 1]].width > width + 1e-6
                {
                    let length: f64 = path[from..=pos]
                        .iter()
                        .map(|&i| b.tracks[i].a.distance(b.tracks[i].b))
                        .sum();
                    if length < c.narrow_run_mm {
                        e.emit("route.width_island",path[from-1..=pos+1].iter().map(|&i|b.tracks[i].id.clone()).collect(),nets(),points[from],"","Short contiguous narrow run between wider tracks; check required clearance".into(),&[("run_mm",length),("width_mm",width)]);
                    }
                }
                pos += 1;
            }
        }
        let ends: Vec<_> = [first, last]
            .iter()
            .map(|p| {
                b.vias
                    .iter()
                    .filter(|v| {
                        v.net == b.tracks[path[0]].net
                            && on_layer(&b.via_layers(v), &b.tracks[path[0]].layer)
                            && v.at.distance(*p) <= v.size / 2. + b.tracks[path[0]].width / 2.
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        if ends[0].len() == 1 && ends[1].len() == 1 && ends[0][0].id != ends[1][0].id {
            let v = ends[0][0];
            let w = ends[1][0];
            let other_layers = |v: &Via| {
                b.tracks
                    .iter()
                    .filter(|t| {
                        t.net == v.net
                            && t.layer != b.tracks[path[0]].layer
                            && line_distance(v.at, t.a, t.b) <= (v.size + t.width) / 2.
                    })
                    .map(|t| t.layer.clone())
                    .collect::<BTreeSet<_>>()
            };
            if !other_layers(v).is_disjoint(&other_layers(w)) {
                let mut all: Vec<String> = ids();
                all.extend([v.id.clone(), w.id.clone()]);
                e.emit("route.layer_excursion",all,nets(),first,"","Via pair leaves and returns to a common layer; search for a legal same-layer route".into(),&[("excursion_mm",len)]);
            }
        }
    }
}

fn contracts(e: &mut Emitter) {
    let b = e.board;
    let c = e.config;
    for rule in &c.net_rules {
        let tracks: Vec<_> = b.tracks.iter().filter(|t| t.net == rule.net).collect();
        let vias: Vec<_> = b.vias.iter().filter(|t| t.net == rule.net).collect();
        if tracks.is_empty() && vias.is_empty() && !b.pads.iter().any(|p| p.net == rule.net) {
            e.emit(
                "contract.missing_net",
                vec![format!("contract:net:{}", rule.net)],
                vec![],
                Point { x: 0., y: 0. },
                &rule.net,
                "Contract references a net absent from the board".into(),
                &[],
            );
        }
        for t in tracks {
            if let Some(w) = rule.min_width_mm.filter(|&w| t.width < w) {
                e.emit(
                    "contract.net_width",
                    vec![t.id.clone()],
                    vec![t.net.clone()],
                    t.a,
                    "",
                    "Width below this net's explicit contract".into(),
                    &[("width_mm", t.width), ("minimum_mm", w)],
                );
            }
            if rule
                .allowed_layers
                .as_ref()
                .is_some_and(|v| !v.contains(&t.layer))
            {
                e.emit(
                    "contract.net_layer",
                    vec![t.id.clone()],
                    vec![t.net.clone()],
                    t.a,
                    "",
                    "Track lies outside explicitly allowed layers".into(),
                    &[],
                );
            }
        }
        if let Some(max) = rule.max_vias.filter(|&max| vias.len() > max) {
            e.emit(
                "contract.via_budget",
                vias.iter().map(|v| v.id.clone()).collect(),
                vec![rule.net.clone()],
                vias[0].at,
                "",
                "Net exceeds explicit via budget".into(),
                &[("vias", vias.len() as f64), ("maximum", max as f64)],
            );
        }
    }
    for r in &c.pin_nets {
        let matches: Vec<_> = b
            .pads
            .iter()
            .filter(|p| p.reference == r.reference && p.number == r.pad)
            .collect();
        if matches.is_empty() {
            e.emit(
                "contract.pin_net",
                vec![format!("contract:pin:{}:{}", r.reference, r.pad)],
                vec![],
                Point { x: 0., y: 0. },
                "",
                format!("Required pad {}.{} is missing", r.reference, r.pad),
                &[],
            );
        }
        for p in matches {
            if p.net != r.net {
                e.emit(
                    "contract.pin_net",
                    vec![p.id.clone()],
                    vec![p.net.clone(), r.net.clone()],
                    p.at,
                    "",
                    format!(
                        "{}.{} expects {}, found {}",
                        r.reference, r.pad, r.net, p.net
                    ),
                    &[],
                );
            }
        }
    }
    for r in &c.proximity {
        let a = b.parts.iter().find(|p| p.reference == r.reference);
        let z = b.parts.iter().find(|p| p.reference == r.to);
        if let (Some(a), Some(z)) = (a, z) {
            let d = a.at.distance(z.at);
            let ids = || vec![a.id.clone(), z.id.clone()];
            if d > r.max_distance_mm {
                e.emit(
                    "contract.proximity",
                    ids(),
                    vec![],
                    a.at,
                    "",
                    format!(
                        "{} is too far from {} by footprint-origin distance",
                        r.reference, r.to
                    ),
                    &[("distance_mm", d), ("maximum_mm", r.max_distance_mm)],
                );
            }
            if r.same_side && a.layer != z.layer {
                e.emit(
                    "contract.side",
                    ids(),
                    vec![],
                    a.at,
                    "",
                    "Contracted components are on different sides".into(),
                    &[],
                );
            }
        } else {
            e.emit(
                "contract.proximity",
                vec![format!("contract:pair:{}:{}", r.reference, r.to)],
                vec![],
                Point { x: 0., y: 0. },
                "",
                "Proximity contract references a missing component".into(),
                &[],
            );
        }
    }
    for g in &c.groups {
        let parts: Vec<_> = b
            .parts
            .iter()
            .filter(|p| g.references.contains(&p.reference))
            .collect();
        if parts.len() != g.references.len() {
            e.emit(
                "contract.alignment",
                vec![format!("contract:group:{}", g.name)],
                vec![],
                Point { x: 0., y: 0. },
                &g.name,
                "Alignment group has missing or ambiguous references".into(),
                &[],
            );
            continue;
        }
        let values: Vec<_> = parts
            .iter()
            .map(|p| if g.axis == "row" { p.at.y } else { p.at.x })
            .collect();
        let spread = values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - values.iter().copied().fold(f64::INFINITY, f64::min);
        if spread > g.tolerance_mm {
            e.emit(
                "contract.alignment",
                parts.iter().map(|p| p.id.clone()).collect(),
                vec![],
                parts[0].at,
                &g.name,
                format!("Group {} deviates from a {}", g.name, g.axis),
                &[("spread_mm", spread)],
            );
        }
    }
}
