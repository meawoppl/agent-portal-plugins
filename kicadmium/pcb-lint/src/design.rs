use crate::{
    Emitter,
    advanced::{contract, emit, part, pin, ready},
    copper::*,
    model::*,
};
use geo::{Area, Intersects, Line};
use petgraph::{algo::has_path_connecting, graphmap::DiGraphMap};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub fn run(e: &mut Emitter, g: &Geometry) {
    loops(e, g);
    schematic(e);
    electrical(e, g);
    power(e);
    interfaces(e);
}
fn path(b: &Board, g: &Geometry, ids: &[String]) -> Option<Vec<Point>> {
    let mut chunks = vec![];
    for id in ids {
        if let Some(t) = b.tracks.iter().find(|t| &t.id == id) {
            chunks.push(vec![t.a, t.b]);
        } else {
            let a = g.extra.arcs.iter().find(|t| &t.id == id)?;
            chunks.push(a.points.clone());
        }
    }
    if chunks.is_empty() {
        return None;
    }
    let mut out = chunks.remove(0);
    if let Some(next) = chunks.first()
        && out[0]
            .distance(next[0])
            .min(out[0].distance(*next.last().unwrap()))
            < out[1]
                .distance(next[0])
                .min(out[1].distance(*next.last().unwrap()))
    {
        out.reverse();
    }
    for mut p in chunks {
        if out.last().unwrap().distance(*p.last().unwrap()) < out.last().unwrap().distance(p[0]) {
            p.reverse();
        }
        if out.last().unwrap().distance(p[0]) > 0.01 {
            return None;
        }
        out.extend(p.into_iter().skip(1));
    }
    Some(out)
}
fn loops(e: &mut Emitter, g: &Geometry) {
    let b = e.board;
    let c = &e.config.intent;
    for id in ["decoupling.pin_distance", "decoupling.loop_area"] {
        ready(e, id, c.loops.iter().any(|l| l.kind == "decoupling"));
    }
    ready(
        e,
        "regulator.feedback",
        c.loops.iter().any(|l| l.kind == "feedback"),
    );
    ready(
        e,
        "regulator.hot_loop",
        c.loops.iter().any(|l| l.kind == "hot_loop"),
    );
    for l in &c.loops {
        let rule = match l.kind.as_str() {
            "decoupling" => "decoupling.pin_distance",
            "feedback" => "regulator.feedback",
            "hot_loop" => "regulator.hot_loop",
            _ => continue,
        };
        let Some(power) = pin(b, &l.power_pin) else {
            contract(e, rule, &l.id, "Assigned power/sense pin missing");
            continue;
        };
        let caps: Vec<_> = b
            .pads
            .iter()
            .filter(|p| p.reference == l.capacitor && p.net == power.net)
            .collect();
        if l.kind == "decoupling" {
            let d = caps
                .iter()
                .map(|p| p.at.distance(power.at))
                .fold(f64::INFINITY, f64::min);
            if d > l.max_pin_mm {
                let mut ids = vec![power.id.clone()];
                ids.extend(caps.iter().map(|p| p.id.clone()));
                emit(
                    e,
                    rule,
                    ids,
                    power.at,
                    &l.id,
                    "Assigned capacitor lacks a nearby same-net pad at the served power pin",
                    &[("distance_mm", d.min(1e9)), ("maximum_mm", l.max_pin_mm)],
                );
            }
        }
        let loop_rule = if l.kind == "decoupling" {
            "decoupling.loop_area"
        } else {
            rule
        };
        if let Some(ps) = path(b, g, &l.path) {
            let len: f64 = ps.windows(2).map(|w| w[0].distance(w[1])).sum();
            let area = polygon(&ps).unsigned_area();
            if l.kind == "feedback" {
                let bad_net = l
                    .path
                    .iter()
                    .filter_map(|id| b.tracks.iter().find(|t| &t.id == id))
                    .any(|t| !l.allowed_nets.contains(&t.net));
                if len > l.max_path_mm || bad_net {
                    emit(
                        e,
                        rule,
                        l.path.clone(),
                        ps[0],
                        &l.id,
                        "Feedback sense path exceeds length or net-role constraints",
                        &[("length_mm", len)],
                    );
                }
            } else if area > l.max_loop_area_mm2 {
                emit(
                    e,
                    loop_rule,
                    l.path.clone(),
                    ps[0],
                    &l.id,
                    "Assigned loop encloses excess area (closure includes direct component chord)",
                    &[("area_mm2", area)],
                );
            }
        } else {
            contract(
                e,
                loop_rule,
                &l.id,
                "Assigned loop path is absent, discontinuous or contains unsupported objects",
            );
        }
    }
}
fn schematic(e: &mut Emitter) {
    let c = &e.config.intent;
    for id in [
        "schematic.collisions",
        "schematic.junction",
        "schematic.stem",
        "schematic.reading_order",
    ] {
        ready(e, id, c.schematic.is_some());
    }
    let Some(s) = &c.schematic else { return };
    for (i, a) in s.items.iter().enumerate() {
        for z in &s.items[i + 1..] {
            if a.sheet != z.sheet
                || a.attach_to.as_ref() == Some(&z.id)
                || z.attach_to.as_ref() == Some(&a.id)
            {
                continue;
            }
            if overlaps(&a.bounds, &z.bounds) {
                emit(
                    e,
                    "schematic.collisions",
                    vec![a.id.clone(), z.id.clone()],
                    a.bounds.min,
                    &a.sheet,
                    "Schematic item envelopes overlap",
                    &[],
                );
            }
        }
    }
    for (i, a) in s.wires.iter().enumerate() {
        let aa = Line::new((a.a.x, a.a.y), (a.b.x, a.b.y));
        for z in &s.wires[i + 1..] {
            if a.sheet != z.sheet {
                continue;
            }
            let zz = Line::new((z.a.x, z.a.y), (z.b.x, z.b.y));
            if let Some(geo::line_intersection::LineIntersection::SinglePoint {
                intersection,
                ..
            }) = geo::line_intersection::line_intersection(aa, zz)
            {
                let p = Point {
                    x: intersection.x,
                    y: intersection.y,
                };
                let dot = s
                    .junctions
                    .iter()
                    .any(|(sheet, q)| sheet == &a.sheet && q.distance(p) < 0.01);
                let both_inner = [a.a, a.b, z.a, z.b].iter().all(|q| q.distance(p) > 0.01);
                if (dot && a.net != z.net) || (both_inner && a.net == z.net && !dot) {
                    emit(
                        e,
                        "schematic.junction",
                        vec![a.id.clone(), z.id.clone()],
                        p,
                        &a.sheet,
                        "Ambiguous same-net crossing without dot, or dot joining different declared nets",
                        &[],
                    );
                }
            }
        }
        let len = a.a.distance(a.b);
        let local = s.items.iter().any(|i| {
            i.sheet == a.sheet
                && ["pin", "label"].contains(&i.kind.as_str())
                && (rect(&i.bounds).intersects(&geo::Point::new(a.a.x, a.a.y))
                    || rect(&i.bounds).intersects(&geo::Point::new(a.b.x, a.b.y)))
        });
        if local && len > s.max_stem_mm {
            emit(
                e,
                "schematic.stem",
                vec![a.id.clone()],
                a.a,
                &a.sheet,
                "Pin/label wire stem exceeds readability budget",
                &[("length_mm", len)],
            );
        }
    }
    for pair in s.reading_order.windows(2) {
        let a = s.items.iter().find(|i| i.id == pair[0]);
        let z = s.items.iter().find(|i| i.id == pair[1]);
        if let (Some(a), Some(z)) = (a, z) {
            if a.sheet == z.sheet
                && (a.bounds.min.y, a.bounds.min.x) > (z.bounds.min.y, z.bounds.min.x)
            {
                emit(
                    e,
                    "schematic.reading_order",
                    vec![a.id.clone(), z.id.clone()],
                    a.bounds.min,
                    &a.sheet,
                    "Declared reading order conflicts with top-to-bottom, left-to-right placement",
                    &[],
                );
            }
        } else {
            contract(
                e,
                "schematic.reading_order",
                &pair[0],
                "Reading-order item missing",
            );
        }
    }
}
fn electrical(e: &mut Emitter, g: &Geometry) {
    let b = e.board;
    let c = &e.config.intent;
    if ready(e, "netlist.parity", c.netlist.is_some()) {
        let expected: BTreeSet<_> = c
            .netlist
            .as_ref()
            .unwrap()
            .iter()
            .map(|p| (&p.reference, &p.pad, &p.net))
            .collect();
        let actual: BTreeSet<_> = b
            .pads
            .iter()
            .filter(|p| !p.net.is_empty())
            .map(|p| (&p.reference, &p.number, &p.net))
            .collect();
        for p in expected.symmetric_difference(&actual) {
            contract(
                e,
                "netlist.parity",
                &format!("{}.{}", p.0, p.1),
                format!(
                    "Schematic/PCB endpoint-set mismatch at {}.{} on {}",
                    p.0, p.1, p.2
                ),
            );
        }
    }
    if ready(e, "connectivity.regression", c.baseline.is_some()) {
        if g.uncertain {
            e.coverage.insert(
                "connectivity.regression".into(),
                "unsupported_geometry".into(),
            );
        } else {
            let expected = canonical(&c.baseline.as_ref().unwrap().connectivity);
            let actual = canonical(&g.components(None));
            for group in expected.symmetric_difference(&actual) {
                emit(
                    e,
                    "connectivity.regression",
                    group.clone(),
                    Point { x: 0., y: 0. },
                    "",
                    "Pad connected-component membership differs from baseline",
                    &[],
                );
            }
        }
    }
    ready(e, "power.width_capacity", !c.currents.is_empty());
    for current in &c.currents {
        // IPC-2221 legacy empirical screening equation, not an IPC-2152 thermal solver.
        let k = if current.internal { 0.024 } else { 0.048 };
        let area_mil2 =
            (current.amperes / (k * current.temperature_rise_c.powf(0.44))).powf(1. / 0.725);
        let width_mm = area_mil2 / (current.copper_um / 25.4) * 0.0254;
        let tracks: Vec<_> = b.tracks.iter().filter(|t| t.net == current.net).collect();
        if tracks.is_empty() {
            contract(
                e,
                "power.width_capacity",
                &current.net,
                "Current-carrying net has no routed straight segments",
            );
        }
        for t in tracks {
            if t.width < width_mm {
                emit(
                    e,
                    "power.width_capacity",
                    vec![t.id.clone()],
                    t.a,
                    &current.net,
                    "Width below legacy IPC-2221 estimate; verify temperature/stackup with thermal analysis",
                    &[("estimate_mm", width_mm), ("actual_mm", t.width)],
                );
            }
        }
    }
    if ready(e, "feature.unrequested", c.features.is_some()) {
        let spec = c.features.as_ref().unwrap();
        for f in &b.parts {
            if !spec.allowed_references.contains(&f.reference) {
                emit(
                    e,
                    "feature.unrequested",
                    vec![f.id.clone()],
                    f.at,
                    "",
                    "Component is outside the explicitly allowed feature inventory",
                    &[],
                );
            }
        }
        for r in &spec.required_references {
            if part(b, r).is_none() {
                contract(e, "feature.unrequested", r, "Required component absent");
            }
        }
    }
    for id in ["pin.swap_candidate", "pin.swap_legality"] {
        ready(e, id, !c.swaps.is_empty());
    }
    for group in &c.swaps {
        for p in &group.pins {
            if pin(b, &p.pin).is_none_or(|pad| pad.net != p.pin.net) {
                contract(
                    e,
                    "pin.swap_legality",
                    &group.name,
                    "Swap assignment pad missing or net differs from reviewed assignment",
                );
            }
            if !p.required.iter().all(|r| p.capabilities.contains(r)) {
                let subjects = pin(b, &p.pin)
                    .map(|p| vec![p.id.clone()])
                    .unwrap_or_else(|| vec![format!("contract:{}", p.pin.reference)]);
                emit(
                    e,
                    "pin.swap_legality",
                    subjects,
                    Point { x: 0., y: 0. },
                    &group.name,
                    "Assigned pin lacks a required capability",
                    &[],
                );
            }
        }
        for (i, a) in group.pins.iter().enumerate() {
            for z in &group.pins[i + 1..] {
                if a.mate_fixed
                    || z.mate_fixed
                    || a.bank != z.bank
                    || (a.voltage - z.voltage).abs() > 0.01
                    || !a.required.iter().all(|r| z.capabilities.contains(r))
                    || !z.required.iter().all(|r| a.capabilities.contains(r))
                {
                    continue;
                }
                let (Some(pa), Some(pz)) = (pin(b, &a.pin), pin(b, &z.pin)) else {
                    contract(e, "pin.swap_legality", &group.name, "Swap-group pad absent");
                    continue;
                };
                let cost = |at: Point, net: &str| {
                    b.pads
                        .iter()
                        .filter(|p| {
                            p.net == net
                                && p.reference != pa.reference
                                && p.reference != pz.reference
                        })
                        .map(|p| at.distance(p.at))
                        .sum::<f64>()
                };
                let old = cost(pa.at, &pa.net) + cost(pz.at, &pz.net);
                let new = cost(pa.at, &pz.net) + cost(pz.at, &pa.net);
                if old - new > group.minimum_saving_mm {
                    emit(
                        e,
                        "pin.swap_candidate",
                        vec![pa.id.clone(), pz.id.clone()],
                        pa.at,
                        &group.name,
                        "Constraint-compatible swap reduces endpoint distance; update schematic and mating contracts together",
                        &[("saving_mm", old - new)],
                    );
                }
            }
        }
    }
    ready(e, "net.naming", !c.naming.is_empty());
    for naming in &c.naming {
        let re = regex::Regex::new(&naming.pattern).expect("validated regex");
        for net in &naming.nets {
            if !re.is_match(net) {
                let ids = b
                    .pads
                    .iter()
                    .filter(|p| &p.net == net)
                    .map(|p| p.id.clone())
                    .collect();
                emit(
                    e,
                    "net.naming",
                    ids,
                    Point { x: 0., y: 0. },
                    net,
                    "Net name does not match explicit naming schema",
                    &[],
                );
            }
        }
    }
    ready(e, "power.pin_roles", !c.pin_roles.is_empty());
    for r in &c.pin_roles {
        let p = pin(b, &r.pin);
        if r.role.is_empty() || r.source.is_empty() || p.is_none_or(|p| p.net != r.pin.net) {
            contract(
                e,
                "power.pin_roles",
                &format!("{}.{}", r.pin.reference, r.pin.pad),
                format!(
                    "Power-role {} has missing provenance, pad or wrong net (expected {})",
                    r.role, r.pin.net
                ),
            );
        }
    }
    ready(e, "strap.truth_table", !c.straps.is_empty());
    for s in &c.straps {
        let pins: Vec<_> = b
            .pads
            .iter()
            .filter(|p| p.reference == s.pull_reference)
            .collect();
        let p = pin(b, &s.pin);
        let value = part(b, &s.pull_reference).and_then(|p| engineering(&p.value));
        let valid = s.source.len() > 3
            && p.is_some_and(|p| pins.iter().any(|r| r.net == p.net))
            && pins.iter().any(|r| r.net == s.target_net)
            && value.is_some_and(|v| v >= s.low_ohm && v <= s.high_ohm);
        if !valid {
            contract(
                e,
                "strap.truth_table",
                &format!("{}.{}", s.pin.reference, s.pin.pad),
                "Strap resistor/net connection or resistance disagrees with reviewed strap-state contract",
            );
        }
    }
}
pub fn canonical(groups: &[Vec<String>]) -> BTreeSet<Vec<String>> {
    groups
        .iter()
        .map(|g| {
            let mut g = g.clone();
            g.sort();
            g.dedup();
            g
        })
        .collect()
}
pub fn engineering(s: &str) -> Option<f64> {
    let s = s.trim().replace('Ω', "").replace("ohm", "");
    if let Ok(v) = s.parse() {
        return Some(v);
    }
    for (suffix, factor) in [
        ("k", 1e3),
        ("K", 1e3),
        ("M", 1e6),
        ("m", 1e-3),
        ("u", 1e-6),
        ("n", 1e-9),
        ("p", 1e-12),
        ("R", 1.),
    ] {
        if let Some((a, z)) = s.split_once(suffix) {
            if z.is_empty() {
                return a.parse::<f64>().ok().map(|v| v * factor);
            }
            if z.chars().all(|c| c.is_ascii_digit()) {
                return format!("{a}.{z}").parse::<f64>().ok().map(|v| v * factor);
            }
        }
    }
    None
}
fn interfaces(e: &mut Emitter) {
    let b = e.board;
    let c = &e.config.intent;
    ready(e, "interface.completeness", !c.interfaces.is_empty());
    for i in &c.interfaces {
        for p in &i.pins {
            if pin(b, p).is_none_or(|a| a.net != p.net) {
                contract(
                    e,
                    "interface.completeness",
                    &format!("{}:{}:{}", i.name, p.reference, p.pad),
                    "Required interface endpoint missing or on wrong net",
                );
            }
        }
        for r in &i.required_references {
            if part(b, r).is_none() {
                contract(
                    e,
                    "interface.completeness",
                    &format!("{}:{r}", i.name),
                    "Required interface component missing",
                );
            }
        }
    }
    ready(e, "connector.mating", !c.mating.is_empty());
    for m in &c.mating {
        for p in &m.expected {
            if pin(b, p).is_none_or(|a| a.net != p.net) {
                contract(
                    e,
                    "connector.mating",
                    &format!("{}:{}:{}", m.name, p.reference, p.pad),
                    "Mating connector pin/net mapping mismatch",
                );
            }
        }
    }
}
fn power(e: &mut Emitter) {
    let b = e.board;
    let c = &e.config.intent;
    for id in [
        "source.series_parallel",
        "source.backfeed",
        "source.polarity",
    ] {
        ready(e, id, !c.modes.is_empty());
    }
    for m in &c.modes {
        let mut graph: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
        let mut directed = DiGraphMap::<&str, ()>::new();
        let mut link = |a: &str, z: &str, v: f64| {
            graph.entry(a.into()).or_default().push((z.into(), v));
            graph.entry(z.into()).or_default().push((a.into(), -v));
        };
        for l in &m.links {
            if l.bidirectional {
                link(&l.from, &l.to, 0.);
            }
            directed.add_edge(l.from.as_str(), l.to.as_str(), ());
            if l.bidirectional {
                directed.add_edge(l.to.as_str(), l.from.as_str(), ());
            }
        }
        for s in &m.sources {
            link(&s.negative, &s.positive, s.volts);
            directed.add_node(&s.positive);
            directed.add_node(&s.negative);
        }
        let mut values: BTreeMap<String, f64> = BTreeMap::new();
        let mut queue = VecDeque::from([(m.output_negative.clone(), 0.)]);
        let mut conflict = false;
        while let Some((net, v)) = queue.pop_front() {
            if let Some(w) = values.get(&net) {
                if (w - v).abs() > m.tolerance_volts {
                    conflict = true
                }
                continue;
            }
            values.insert(net.clone(), v);
            for (z, d) in graph.get(&net).into_iter().flatten() {
                queue.push_back((z.clone(), v + d));
            }
        }
        let voltage = values.get(&m.output_positive);
        let isolated_ok = m.sources.iter().enumerate().all(|(i, a)| {
            m.sources[i + 1..].iter().all(|z| {
                a.isolation_group.is_empty()
                    || a.isolation_group != z.isolation_group
                    || a.negative == z.negative
                    || values
                        .get(&a.negative)
                        .zip(values.get(&z.negative))
                        .is_some_and(|(a, b)| (a - b).abs() <= m.tolerance_volts)
            })
        });
        if m.sources.len() > 1 && m.sources.iter().any(|s| s.isolation_group.is_empty()) {
            e.coverage
                .insert("source.series_parallel".into(), "needs_input".into());
        }
        if conflict
            || !isolated_ok
            || voltage.is_none_or(|v| (v - m.target_volts).abs() > m.tolerance_volts)
        {
            contract(
                e,
                "source.series_parallel",
                &m.name,
                format!(
                    "Source mode has inconsistent constraints, shared-ground series conflict or wrong output: {:?} V, expected {} V",
                    voltage, m.target_volts
                ),
            );
        }
        if !m.allow_parallel {
            for (i, a) in m.sources.iter().enumerate() {
                for z in &m.sources[i + 1..] {
                    let reach =
                        |a: &str, b: &str| a == b || has_path_connecting(&directed, a, b, None);
                    if reach(&a.negative, &z.negative)
                        && reach(&z.negative, &a.negative)
                        && (reach(&a.positive, &z.positive) || reach(&z.positive, &a.positive))
                    {
                        contract(
                            e,
                            "source.backfeed",
                            &format!("{}:{}:{}", m.name, a.name, z.name),
                            "Source positives have a conducting path with common return; inspect backfeed protection",
                        );
                    }
                }
            }
        }
        for p in &m.connector_pins {
            if pin(b, p).is_none_or(|q| q.net != p.net) {
                contract(
                    e,
                    "source.polarity",
                    &format!("{}:{}:{}", m.name, p.reference, p.pad),
                    "Operating-mode connector polarity/net contract mismatch",
                );
            }
        }
    }
}
