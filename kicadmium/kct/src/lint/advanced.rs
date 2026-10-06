use crate::lint::{copper::*, intent::*, model::*, Emitter};
use anyhow::Result;
use geo::{Contains, Intersects};
use petgraph::graph::NodeIndex;
use std::collections::{BTreeMap, BTreeSet};
pub const IDS: &[&str] = &[
    "copper.island",
    "copper.stale_stub",
    "copper.duplicate_overlap",
    "via.avoidable",
    "via.role_missing",
    "via.return_distance",
    "route.legal_shortcut",
    "route.redundant_bend",
    "route.backtrack",
    "pad.grazing",
    "route.tuning_integrity",
    "placement.airwire_cost",
    "placement.congestion",
    "placement.rotation",
    "placement.pitch",
    "placement.orientation",
    "channel.topology",
    "channel.geometry",
    "channel.polarity",
    "block.flow",
    "power.corridor",
    "noise.partition",
    "decoupling.pin_distance",
    "decoupling.loop_area",
    "regulator.feedback",
    "regulator.hot_loop",
    "schematic.collisions",
    "schematic.junction",
    "schematic.stem",
    "schematic.reading_order",
    "netlist.parity",
    "connectivity.regression",
    "power.width_capacity",
    "neck.clearance_reason",
    "width.taper",
    "feature.unrequested",
    "pin.swap_candidate",
    "pin.swap_legality",
    "net.naming",
    "interface.completeness",
    "connector.mating",
    "testpoint.coverage",
    "testpoint.access",
    "testpoint.ground",
    "testpoint.label",
    "pair.skew",
    "pair.spacing",
    "reference.discontinuity",
    "rf.launch",
    "power.pin_roles",
    "strap.truth_table",
    "source.series_parallel",
    "source.backfeed",
    "source.polarity",
    "capacitor.role",
    "capacitor.rating",
    "parts.package",
    "mechanics.edge_clearance",
    "mechanics.access",
    "thermal.spokes",
    "bom.completeness",
    "cpl.orientation",
    "silk.collisions",
    "silk.content",
    "export.freshness",
    "release.manifest",
    "checker.transform",
    "checker.differential",
    "checker.noise",
    "review.reattach",
    "annotation.stale",
];
pub fn ready(e: &mut Emitter, id: &str, ok: bool) -> bool {
    e.coverage.insert(
        id.into(),
        if ok { "evaluated" } else { "needs_input" }.into(),
    );
    ok
}
pub fn emit(
    e: &mut Emitter,
    id: &str,
    subjects: Vec<String>,
    at: Point,
    slot: &str,
    msg: impl Into<String>,
    metrics: &[(&str, f64)],
) {
    let nets = e
        .board
        .tracks
        .iter()
        .filter(|t| subjects.contains(&t.id))
        .map(|t| t.net.clone())
        .chain(
            e.board
                .pads
                .iter()
                .filter(|t| subjects.contains(&t.id))
                .map(|t| t.net.clone()),
        )
        .chain(
            e.board
                .vias
                .iter()
                .filter(|t| subjects.contains(&t.id))
                .map(|t| t.net.clone()),
        )
        .collect();
    e.emit(id, subjects, nets, at, slot, msg.into(), metrics);
}
pub fn contract(e: &mut Emitter, id: &str, key: &str, msg: impl Into<String>) {
    emit(
        e,
        id,
        vec![format!("contract:{key}")],
        Point { x: 0., y: 0. },
        key,
        msg,
        &[],
    )
}
pub fn pin<'a>(b: &'a Board, p: &Pin) -> Option<&'a Pad> {
    b.pads
        .iter()
        .find(|q| q.reference == p.reference && q.number == p.pad)
}
pub fn part<'a>(b: &'a Board, r: &str) -> Option<&'a Part> {
    b.parts.iter().find(|p| p.reference == r)
}
pub fn angle(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.);
    d.min(360. - d)
}
pub fn run(e: &mut Emitter, input: &str) -> Result<()> {
    let extra = Extra::read(input, e.board)?;
    let g = Geometry::new(e.board, extra, &e.config.intent.route);
    for id in IDS {
        e.coverage.insert((*id).into(), "needs_input".into());
    }
    via_attachment(e, &g);
    screen_layer_excursions(e, &g);
    via_overshoot(e, &g);
    geometry(e, &g);
    passive_alignment(e);
    placement(e, &g);
    crate::lint::design::run(e, &g);
    crate::lint::manufacturing::run(e, &g);
    Ok(())
}
// Screen the existing path moved to a common departure/return layer, without
// inventing an alternate detour. A blocked excursion is not a useful suggestion.
fn screen_layer_excursions(e: &mut Emitter, g: &Geometry) {
    let b = e.board;
    let policy = &e.config.intent.route;
    let uncertain = g.extra.incomplete.iter().any(|x| {
        !x.contains("Edge.Cuts")
            && !x.starts_with("unfilled zone ")
            && !x.starts_with("unsupported or invalid filled polygon ")
    });
    let findings = std::mem::take(&mut e.findings);
    let mut withheld = false;
    for f in findings {
        if f.rule != "route.layer_excursion" {
            e.findings.push(f);
            continue;
        }
        if uncertain {
            withheld = true;
            continue;
        }
        let chain: Vec<_> = b
            .tracks
            .iter()
            .filter(|t| f.subjects.contains(&t.id))
            .collect();
        let vias: Vec<_> = b
            .vias
            .iter()
            .filter(|v| f.subjects.contains(&v.id))
            .collect();
        if chain.is_empty() || vias.len() != 2 {
            continue;
        }
        let target = b.copper_layers.iter().find(|layer| {
            **layer != chain[0].layer
                && vias.iter().all(|v| {
                    b.via_layers(v).contains(layer)
                        && b.tracks.iter().any(|t| {
                            t.net == v.net
                                && &t.layer == *layer
                                && line_distance(v.at, t.a, t.b) <= (v.size + t.width) / 2.
                        })
                })
                && {
                    let missing_fill = g.extra.zones.iter().any(|z| {
                        &z.layer == *layer
                            && z.net != chain[0].net
                            && (z.filled.is_empty() || !z.fill_supported)
                    });
                    withheld |= missing_fill;
                    !missing_fill
                }
                && chain.iter().all(|t| {
                    let path = [t.a, t.b];
                    if g.outline.is_some() {
                        g.legal(&path, t.width, &t.net, layer, policy)
                    } else {
                        g.clearance_free(&path, t.width, &t.net, layer, policy)
                    }
                })
        });
        if let Some(layer) = target {
            let metrics: Vec<_> = f.metrics.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            e.emit(&f.rule, f.subjects, f.nets, f.at, "",
                format!("Via excursion could move its existing path to {layer} without modeled foreign-copper collisions at configured clearance; verify with native DRC"),
                &metrics);
        }
    }
    e.coverage.insert(
        "route.layer_excursion".into(),
        if withheld {
            "partial_unsupported_geometry"
        } else {
            "evaluated"
        }
        .into(),
    );
}
// Filled copper is the source of connectivity, never a zone's intended outline.
// Missing/invalid fills only make vias on the same net and layers uncertain.
fn via_attachment(e: &mut Emitter, g: &Geometry) {
    let mut invalid = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for z in &g.extra.zones {
        if z.filled.is_empty() || !z.fill_supported {
            unknown.insert((z.net.clone(), z.layer.clone()));
        }
        for (i, _) in z.filled.iter().enumerate() {
            if !z.fill_supported {
                invalid.insert(format!("{}:fill:{i}:{}", z.id, z.layer));
                unknown.insert((z.net.clone(), z.layer.clone()));
            }
        }
    }
    // Unexpanded copper can add contacts; Edge.Cuts and keepouts cannot.
    let unknown_copper = g.extra.incomplete.iter().any(|s| {
        s.contains("footprint zone")
            || s.contains("unsupported pad shape")
            || s.contains("graphics")
    });
    let mut withheld = false;
    for (i, v) in g.items.iter().enumerate().filter(|(_, x)| x.kind == "via") {
        let connected: BTreeSet<_> = g
            .graph
            .neighbors(NodeIndex::new(i))
            .map(|n| &g.items[n.index()])
            .filter(|x| {
                matches!(x.kind.as_str(), "track" | "arc" | "pad" | "zone")
                    && !invalid.contains(&x.id)
            })
            .flat_map(|x| x.layers.iter().filter(|l| v.layers.contains(l)).cloned())
            .collect();
        if connected.len() >= 2 {
            continue;
        }
        if unknown_copper
            || v.layers
                .iter()
                .any(|l| unknown.contains(&(v.net.clone(), l.clone())))
        {
            withheld = true;
            continue;
        }
        e.emit("via.low_attachment", vec![v.id.clone()], vec![v.net.clone()], v.at, "",
            "Via touches same-net tracks, arcs, pads or saved filled pours on fewer than two copper layers; review its intended connection".into(),
            &[("modeled_layers", connected.len() as f64)]);
    }
    e.coverage.insert(
        "via.low_attachment".into(),
        if withheld {
            "partial_unsupported_geometry"
        } else {
            "evaluated"
        }
        .into(),
    );
}
fn geometry(e: &mut Emitter, g: &Geometry) {
    let b = e.board;
    let c = e.config;
    let p = &c.intent.route;
    for id in [
        "copper.island",
        "copper.stale_stub",
        "copper.duplicate_overlap",
        "pad.grazing",
        "route.backtrack",
        "width.taper",
        "via.role_missing",
    ] {
        ready(e, id, true);
    }
    if g.uncertain {
        for id in ["copper.island", "copper.stale_stub"] {
            e.coverage.insert(id.into(), "unsupported_geometry".into());
        }
    }
    if !g.uncertain {
        for group in g.islands() {
            if group
                .iter()
                .all(|&i| g.items[i].kind != "pad" && g.items[i].kind != "hole")
            {
                let objects: Vec<_> = group.iter().map(|&i| &g.items[i]).collect();
                if objects.iter().any(|x| c.intent.roles.contains_key(&x.id)) {
                    continue;
                }
                emit(
                    e,
                    "copper.island",
                    objects.iter().map(|x| x.id.clone()).collect(),
                    objects[0].at,
                    "",
                    "Copper component has no connected pad or assigned copper role; inspect before removal",
                    &[("objects", objects.len() as f64)],
                );
            }
        }
        let components = g.components(None);
        for (i, item) in g.items.iter().enumerate() {
            if !["track", "arc"].contains(&item.kind.as_str())
                || c.intent.roles.contains_key(&item.id)
            {
                continue;
            }
            if g.graph.neighbors(NodeIndex::new(i)).count() == 1
                && components == g.components(Some(&item.id))
            {
                emit(
                    e,
                    "copper.stale_stub",
                    vec![item.id.clone()],
                    item.at,
                    "",
                    "Leaf copper can be removed without changing modeled pad equivalence classes; verify intended analog/RF function",
                    &[],
                );
            }
        }
    }
    for (i, a) in b.tracks.iter().enumerate() {
        for z in &b.tracks[i + 1..] {
            if a.net != z.net || a.layer != z.layer {
                continue;
            }
            let d = a.b.minus(a.a);
            let q = z.b.minus(z.a);
            if (d.x * q.y - d.y * q.x).abs() < 1e-8 && line_distance(z.a, a.a, a.b) < 1e-6
                || (d.x * q.y - d.y * q.x).abs() < 1e-8 && line_distance(a.a, z.a, z.b) < 1e-6
            {
                let da = a.a.distance(a.b);
                if da < 1e-8 {
                    continue;
                }
                let project = |v: Point| ((v.x - a.a.x) * d.x + (v.y - a.a.y) * d.y) / da;
                let lo = project(z.a).min(project(z.b)).max(0.);
                let hi = project(z.a).max(project(z.b)).min(da);
                if hi - lo > c.join_tolerance_mm {
                    emit(
                        e,
                        "copper.duplicate_overlap",
                        vec![a.id.clone(), z.id.clone()],
                        a.a,
                        "",
                        "Collinear tracks overlap over a finite length",
                        &[("overlap_mm", hi - lo)],
                    );
                }
            }
        }
    }
    for f in e
        .findings
        .clone()
        .iter()
        .filter(|f| f.rule == "trace.width_transition")
    {
        let lo = f.metrics["narrow_mm"];
        let hi = f.metrics["wide_mm"];
        if hi / lo > p.width_ratio {
            emit(
                e,
                "width.taper",
                f.subjects.clone(),
                f.at,
                "",
                "Abrupt width ratio exceeds configured transition limit",
                &[("ratio", hi / lo)],
            );
        }
    }
    for v in &b.vias {
        if !c.intent.roles.contains_key(&v.id) {
            let touches_plane = g.extra.zones.iter().any(|z| {
                z.net == v.net
                    && z.filled
                        .iter()
                        .any(|p| polygon(p).contains(&geo::Point::new(v.at.x, v.at.y)))
            });
            if touches_plane {
                emit(
                    e,
                    "via.role_missing",
                    vec![v.id.clone()],
                    v.at,
                    "",
                    "Plane-connected via lacks a declared thermal/return/stitching role",
                    &[],
                );
            }
        }
    }
    for t in &b.tracks {
        for pad in &b.pads {
            if t.net != pad.net || !on_layer(&pad.layers, &t.layer) {
                continue;
            }
            let pp = pad_poly(pad);
            let copper = line(&[t.a, t.b], t.width);
            if pp.intersects(&copper) && pad_distance(t.a, pad) > 0. && pad_distance(t.b, pad) > 0.
            {
                let thin = line(&[t.a, t.b], (t.width - 2. * p.grazing_margin_mm).max(0.001));
                if !thin.intersects(&pp) {
                    emit(
                        e,
                        "pad.grazing",
                        vec![t.id.clone(), pad.id.clone()],
                        pad.at,
                        "",
                        "Trace barely touches pad envelope without a centerline endpoint landing",
                        &[],
                    );
                }
            }
        }
    }
    // Detect directional backtracking relative to each chain's start-to-end axis, via trace graph.
    let chains = route_chains(b, c.join_tolerance_mm);
    for (ids, points) in &chains {
        let a = points[0];
        let z = *points.last().unwrap();
        let d = z.minus(a);
        let direct = a.distance(z);
        if direct < 0.001 {
            continue;
        }
        let backward: f64 = points
            .windows(2)
            .map(|w| {
                let v = w[1].minus(w[0]);
                (-(v.x * d.x + v.y * d.y) / direct).max(0.)
            })
            .sum();
        if backward > p.minimum_saving_mm {
            emit(
                e,
                "route.backtrack",
                ids.clone(),
                a,
                "",
                "Chain reverses direction relative to its endpoint axis; inspect intentional tuning",
                &[("backtrack_mm", backward)],
            );
        }
    }
    let route_ok = g.outline.is_some() && !g.uncertain;
    ready(e, "route.legal_shortcut", route_ok);
    ready(e, "route.redundant_bend", route_ok);
    ready(e, "via.avoidable", route_ok);
    ready(e, "neck.clearance_reason", route_ok);
    let mut searches = 0;
    if route_ok {
        local_shortcuts(e, g, &chains);
        redundant_bends(e, g, &chains);
        for f in e
            .findings
            .clone()
            .iter()
            .filter(|f| f.rule == "route.layer_excursion")
        {
            let vias: Vec<_> = b
                .vias
                .iter()
                .filter(|v| f.subjects.contains(&v.id))
                .collect();
            if vias.len() != 2 {
                continue;
            }
            let chain: Vec<_> = b
                .tracks
                .iter()
                .filter(|t| f.subjects.contains(&t.id))
                .collect();
            if chain.is_empty() || p.tuned_nets.contains(&chain[0].net) {
                continue;
            }
            for layer in b.copper_layers.iter().filter(|l| {
                *l != &chain[0].layer
                    && vias.iter().all(|v| b.via_layers(v).contains(l))
                    && vias.iter().all(|v| {
                        b.tracks.iter().any(|t| {
                            &t.layer == *l
                                && t.net == v.net
                                && (t.a.distance(v.at) < 0.001 || t.b.distance(v.at) < 0.001)
                        })
                    })
            }) {
                if searches >= 32 {
                    e.coverage
                        .insert("via.avoidable".into(), "budget_exhausted".into());
                    break;
                }
                searches += 1;
                let width = chain.iter().map(|t| t.width).fold(0., f64::max);
                let (path, exhausted) =
                    g.shortcut(vias[0].at, vias[1].at, width, &vias[0].net, layer, p);
                if exhausted {
                    e.coverage
                        .insert("via.avoidable".into(), "budget_exhausted".into());
                }
                if let Some(path) = path {
                    let length: f64 = path.windows(2).map(|p| p[0].distance(p[1])).sum();
                    if length <= f.metrics["excursion_mm"]
                        && vias.iter().all(|v| !c.intent.roles.contains_key(&v.id))
                    {
                        emit(
                            e,
                            "via.avoidable",
                            f.subjects.clone(),
                            vias[0].at,
                            layer,
                            format!(
                                "Candidate route on {layer} could avoid this via excursion; verify all attached branches"
                            ),
                            &[("candidate_mm", length)],
                        );
                    }
                }
            }
        }
        for f in e
            .findings
            .clone()
            .iter()
            .filter(|f| f.rule == "route.width_island")
        {
            let ts: Vec<_> = b
                .tracks
                .iter()
                .filter(|t| f.subjects.contains(&t.id))
                .collect();
            let max = ts.iter().map(|t| t.width).fold(0., f64::max);
            if ts
                .iter()
                .all(|t| g.legal(&[t.a, t.b], max, &t.net, &t.layer, p))
            {
                emit(
                    e,
                    "neck.clearance_reason",
                    f.subjects.clone(),
                    f.at,
                    "",
                    "The entire neck passes geometric widening screen at the adjacent width",
                    &[("candidate_width_mm", max)],
                );
            }
        }
    }
    if ready(e, "route.tuning_integrity", c.intent.baseline.is_some()) {
        let base = c.intent.baseline.as_ref().unwrap();
        for (net, before) in &base.lengths {
            let after = route_length(b, &g.extra, net);
            if (after - before).abs() > base.max_length_change_mm {
                let ids = b
                    .tracks
                    .iter()
                    .filter(|t| &t.net == net)
                    .map(|t| t.id.clone())
                    .collect();
                emit(
                    e,
                    "route.tuning_integrity",
                    ids,
                    Point { x: 0., y: 0. },
                    net,
                    "Net copper length changed beyond baseline budget; branched nets require path-specific interpretation",
                    &[("before_mm", *before), ("after_mm", after)],
                );
            }
        }
    }
    if ready(e, "via.return_distance", !c.intent.references.is_empty()) {
        for r in &c.intent.references {
            for v in b.vias.iter().filter(|v| v.net == r.net) {
                let d = b
                    .vias
                    .iter()
                    .filter(|v| v.net == r.plane_net && on_layer(&b.via_layers(v), &r.plane_layer))
                    .map(|q| q.at.distance(v.at))
                    .fold(f64::INFINITY, f64::min);
                if d > p.max_return_via_mm {
                    emit(
                        e,
                        "via.return_distance",
                        vec![v.id.clone()],
                        v.at,
                        &r.net,
                        "Signal via lacks a nearby via to its specified reference net/layer",
                        &[("distance_mm", d.min(1e9))],
                    );
                }
            }
        }
    }
}
/// Search consecutive subchains, preserving contacts with copper outside the replacement.
fn local_shortcuts(e: &mut Emitter, g: &Geometry, chains: &[(Vec<String>, Vec<Point>)]) {
    let b = e.board;
    let p = &e.config.intent.route;
    let rule = "route.legal_shortcut";
    let mut router_calls = 0;
    let mut windows = 0;
    let mut claimed = BTreeSet::new();
    for (ids, points) in chains {
        if ids.len() < 2 {
            continue;
        }
        let t = b.tracks.iter().find(|t| t.id == ids[0]).unwrap();
        if p.tuned_nets.contains(&t.net)
            || e.config
                .intent
                .pairs
                .iter()
                .any(|q| q.positive == t.net || q.negative == t.net)
        {
            continue;
        }
        // Smallest useful subchains first: local subjects make review and crops precise.
        let sizes: Vec<_> = (2..=ids.len().min(8))
            .chain((ids.len() > 8).then_some(ids.len()))
            .collect();
        for size in sizes {
            for start in 0..=ids.len() - size {
                let selected = &ids[start..start + size];
                if selected.iter().any(|id| claimed.contains(id)) {
                    continue;
                }
                windows += 1;
                if windows > 4096 {
                    e.coverage.insert(rule.into(), "budget_exhausted".into());
                    return;
                }
                let ps = &points[start..=start + size];
                let a = ps[0];
                let z = ps[size];
                let original: f64 = ps.windows(2).map(|w| w[0].distance(w[1])).sum();
                let dx = z.x - a.x;
                let dy = z.y - a.y;
                let octile = dx.abs().max(dy.abs()) + (2_f64.sqrt() - 1.) * dx.abs().min(dy.abs());
                if original - octile <= p.minimum_saving_mm {
                    continue;
                }
                let width = selected
                    .iter()
                    .filter_map(|id| b.tracks.iter().find(|t| &t.id == id))
                    .map(|t| t.width)
                    .fold(0., f64::max);
                let m = dx.abs().min(dy.abs());
                let mut simple = vec![
                    vec![
                        a,
                        Point {
                            x: a.x + dx.signum() * m,
                            y: a.y + dy.signum() * m,
                        },
                        z,
                    ],
                    vec![
                        a,
                        Point {
                            x: z.x - dx.signum() * m,
                            y: z.y - dy.signum() * m,
                        },
                        z,
                    ],
                    vec![a, Point { x: a.x, y: z.y }, z],
                    vec![a, Point { x: z.x, y: a.y }, z],
                ];
                for path in &mut simple {
                    path.dedup_by(|a, b| a.distance(*b) < 1e-9);
                }
                let acceptable = |path: &[Point]| {
                    let length: f64 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
                    if original - length <= p.minimum_saving_mm
                        || !g.legal(path, width, &t.net, &t.layer, p)
                    {
                        return false;
                    }
                    let replacement = line(path, width);
                    selected.iter().all(|id| {
                        let Some(i) = g.items.iter().position(|i| &i.id == id) else {
                            return false;
                        };
                        g.graph.neighbors(NodeIndex::new(i)).all(|n| {
                            selected.contains(&g.items[n.index()].id)
                                || replacement.intersects(&g.items[n.index()].poly)
                        })
                    })
                };
                let mut candidate = simple.into_iter().find(|path| acceptable(path));
                if candidate.is_none() {
                    if router_calls >= 32 {
                        e.coverage.insert(rule.into(), "budget_exhausted".into());
                        continue; // Cheap windows later in the board still get checked.
                    }
                    router_calls += 1;
                    let (path, exhausted) = g.shortcut(a, z, width, &t.net, &t.layer, p);
                    if exhausted {
                        e.coverage.insert(rule.into(), "budget_exhausted".into());
                    }
                    candidate = path.filter(|path| acceptable(path));
                }
                if let Some(path) = candidate {
                    let length: f64 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
                    let coordinates = path
                        .iter()
                        .map(|q| format!("({:.3}, {:.3})", q.x, q.y))
                        .collect::<Vec<_>>()
                        .join(" → ");
                    emit(
                        e,
                        rule,
                        selected.to_vec(),
                        a,
                        "",
                        format!(
                            "Replace {} consecutive segments on {} via {coordinates} mm: saves {:.3} mm; contacts preserved and clearance-screened, native DRC required",
                            size,
                            t.layer,
                            original - length
                        ),
                        &[
                            ("original_mm", original),
                            ("candidate_mm", length),
                            ("saved_mm", original - length),
                            ("segments", size as f64),
                        ],
                    );
                    claimed.extend(selected.iter().cloned());
                }
            }
        }
    }
}

/// Turns along consecutive legs: (count, sharpest turn in degrees). Parallel
/// same-direction legs merge; zero-length legs are ignored.
fn bends(legs: &[Point], tolerance_deg: f64) -> (usize, f64) {
    let sin = tolerance_deg.to_radians().sin();
    let mut previous: Option<Point> = None;
    let (mut count, mut sharpest) = (0, 0_f64);
    for v in legs {
        let length = v.x.hypot(v.y);
        if length < 1e-9 {
            continue;
        }
        if let Some(u) = previous {
            let norm = u.x.hypot(u.y) * length;
            let cross = (u.x * v.y - u.y * v.x) / norm;
            let dot = (u.x * v.x + u.y * v.y) / norm;
            if cross.abs() > sin || dot < 0. {
                count += 1;
                sharpest = sharpest.max(dot.clamp(-1., 1.).acos().to_degrees());
            }
        }
        previous = Some(*v);
    }
    (count, sharpest)
}

/// Same-length reorderings of a chain's legs that remove bends. Moving leg
/// `k` to position `m` translates the legs between them by that leg: the
/// route takes the other two sides of the parallelogram they span, so both
/// endpoints, every leg direction and the total length are unchanged.
fn redundant_bends(e: &mut Emitter, g: &Geometry, chains: &[(Vec<String>, Vec<Point>)]) {
    let b = e.board;
    let p = &e.config.intent.route;
    let tolerance = e.config.angle_tolerance_deg;
    // Smaller jogs are off-grid jitter, reported by `trace.short_segment`.
    let minimum = e.config.short_segment_mm;
    let rule = "route.redundant_bend";
    // A legal shortcut is strictly better advice for the same copper.
    let shortcut: BTreeSet<String> = e
        .findings
        .iter()
        .filter(|f| f.rule == "route.legal_shortcut")
        .flat_map(|f| f.subjects.clone())
        .collect();
    let mut checks = 0;
    for (ids, points) in chains {
        let n = ids.len();
        if !(3..=64).contains(&n) || ids.iter().any(|id| shortcut.contains(id)) {
            continue;
        }
        let tracks: Vec<_> = ids
            .iter()
            .filter_map(|id| b.tracks.iter().find(|t| &t.id == id))
            .collect();
        if tracks.len() != n {
            continue;
        }
        let t = tracks[0];
        if p.tuned_nets.contains(&t.net)
            || e.config
                .intent
                .pairs
                .iter()
                .any(|q| q.positive == t.net || q.negative == t.net)
        {
            continue;
        }
        let legs: Vec<Point> = points.windows(2).map(|w| w[1].minus(w[0])).collect();
        let (before, sharpest) = bends(&legs, tolerance);
        if before < 2 {
            continue;
        }
        // A monotone dogleg (E, SE, E) is good routing: only a chain that
        // backtracks along an axis (the zag) may change an end direction.
        let zag = [|v: &Point| v.x, |v: &Point| v.y].iter().any(|axis| {
            legs.iter().any(|v| axis(v) > 1e-6) && legs.iter().any(|v| axis(v) < -1e-6)
        });
        // (bends removed, swept area, keeps both end directions, k, m, path)
        let mut candidates = vec![];
        for k in 0..n {
            for m in 0..n {
                if k == m {
                    continue;
                }
                let mut order = legs.clone();
                let leg = order.remove(k);
                order.insert(m, leg);
                let (after, worst) = bends(&order, tolerance);
                if after >= before || worst > sharpest + tolerance {
                    continue;
                }
                let (lo, hi) = (k.min(m), k.max(m));
                // Moved copper keeps its width only when the span is uniform.
                if tracks[lo..=hi]
                    .iter()
                    .any(|x| (x.width - t.width).abs() > 1e-6 || x.layer != t.layer)
                {
                    continue;
                }
                let mut path = vec![points[0]];
                for v in &order {
                    let q = *path.last().unwrap();
                    path.push(Point {
                        x: q.x + v.x,
                        y: q.y + v.y,
                    });
                }
                let run = legs[lo..=hi]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| lo + i != k)
                    .fold(Point { x: 0., y: 0. }, |a, (_, v)| Point {
                        x: a.x + v.x,
                        y: a.y + v.y,
                    });
                if leg.x.hypot(leg.y) < minimum || run.x.hypot(run.y) < minimum {
                    continue;
                }
                let area = (leg.x * run.y - leg.y * run.x).abs();
                let ends = order[0] == legs[0] && order[n - 1] == legs[n - 1];
                if !ends && !zag {
                    continue;
                }
                candidates.push((before - after, area, ends, k, m, path));
            }
        }
        candidates.sort_by(|a, z| z.0.cmp(&a.0).then(z.2.cmp(&a.2)).then(a.1.total_cmp(&z.1)));
        let width = tracks.iter().map(|x| x.width).fold(0., f64::max);
        let found = candidates.into_iter().find(|(_, _, _, k, m, path)| {
            checks += 1;
            let (lo, hi) = (*k.min(m), *k.max(m));
            if checks > 4096 || !g.legal(&path[lo..=hi + 1], width, &t.net, &t.layer, p) {
                return false;
            }
            // Every via, pad or branch touching the chain must still be reached.
            let replacement = line(path, width);
            ids.iter().all(|id| {
                let Some(i) = g.items.iter().position(|i| &i.id == id) else {
                    return false;
                };
                g.graph.neighbors(NodeIndex::new(i)).all(|n| {
                    ids.contains(&g.items[n.index()].id)
                        || replacement.intersects(&g.items[n.index()].poly)
                })
            })
        });
        if checks > 4096 {
            e.coverage.insert(rule.into(), "budget_exhausted".into());
            return;
        }
        let Some((removed, area, ends, k, m, path)) = found else {
            continue;
        };
        let (lo, hi) = (k.min(m), k.max(m));
        let leg = legs[k];
        let coordinates = path[lo..=hi + 1]
            .iter()
            .map(|q| format!("({:.3}, {:.3})", q.x, q.y))
            .collect::<Vec<_>>()
            .join(" → ");
        let exit = if ends {
            ""
        } else {
            "; changes the direction leaving a chain end, so check the pad exit"
        };
        emit(
            e,
            rule,
            ids[lo..=hi].to_vec(),
            points[k],
            "",
            format!(
                "Move the {:.3} mm leg {} {} on {}: route {coordinates} mm removes {removed} of {before} bends at the same length; contacts preserved and clearance-screened{exit}, native DRC required",
                leg.x.hypot(leg.y),
                if m < k { "back" } else { "forward" },
                if hi - lo == 1 { "one leg".to_string() } else { format!("{} legs", hi - lo) },
                t.layer,
            ),
            &[
                ("bends_before", before as f64),
                ("bends_after", (before - removed) as f64),
                ("moved_leg_mm", leg.x.hypot(leg.y)),
                ("swept_area_mm2", area),
            ],
        );
    }
}

/// Conservative local relocation: two straight branches, fixed far endpoints, no new bends.
fn via_overshoot(e: &mut Emitter, g: &Geometry) {
    let rule = "via.overshoot";
    let b = e.board;
    let p = &e.config.intent.route;
    if !ready(e, rule, g.outline.is_some() && !g.uncertain) {
        return;
    }
    let tol = e.config.join_tolerance_mm;
    for v in &b.vias {
        if p.tuned_nets.contains(&v.net)
            || e.config
                .intent
                .pairs
                .iter()
                .any(|q| q.positive == v.net || q.negative == v.net)
        {
            continue;
        }
        let Some(vi) = g.items.iter().position(|i| i.id == v.id) else {
            continue;
        };
        let neighbors: Vec<_> = g
            .graph
            .neighbors(NodeIndex::new(vi))
            .map(|i| i.index())
            .collect();
        // Plane, pad, extra branch or arc attachment requires a richer relocation contract.
        if neighbors.len() != 2 {
            continue;
        }
        let branches: Vec<_> = neighbors
            .iter()
            .filter_map(|i| {
                let t = b.tracks.iter().find(|t| t.id == g.items[*i].id)?;
                let end = if t.a.distance(v.at) <= tol {
                    t.b
                } else if t.b.distance(v.at) <= tol {
                    t.a
                } else {
                    return None;
                };
                Some((t, end, *i))
            })
            .collect();
        if branches.len() != 2 || branches[0].0.layer == branches[1].0.layer {
            continue;
        }
        let a = branches[0].1;
        let z = branches[1].1;
        let original = a.distance(v.at) + z.distance(v.at);
        // Intersect the horizontal, vertical and diagonal lines through each anchor.
        let directions = [(1., 0.), (0., 1.), (1., 1.), (1., -1.)];
        let mut candidates = vec![a, z];
        for (dx, dy) in directions {
            for (ex, ey) in directions {
                let det: f64 = dx * ey - dy * ex;
                if det.abs() < 0.1 {
                    continue;
                }
                let t = ((z.x - a.x) * ey - (z.y - a.y) * ex) / det;
                candidates.push(Point {
                    x: a.x + t * dx,
                    y: a.y + t * dy,
                });
            }
        }
        let mut best = None;
        let mut best_len = original - p.minimum_saving_mm;
        for q in candidates {
            let length = a.distance(q) + z.distance(q);
            if length >= best_len || q.distance(v.at) <= tol {
                continue;
            }
            if branches.iter().any(|(t, end, _)| {
                let dx = (q.x - end.x).abs();
                let dy = (q.y - end.y).abs();
                (dx > tol && dy > tol && (dx - dy).abs() > tol)
                    || !g.legal(&[*end, q], t.width, &v.net, &t.layer, p)
            }) {
                continue;
            }
            // The via barrel/annulus must fit on every layer in its span, including inner layers.
            let envelope = disk(q, v.size / 2. + p.clearance_mm + 0.005);
            if !g
                .outline
                .as_ref()
                .unwrap()
                .contains(&disk(q, v.size / 2. + p.edge_clearance_mm + 0.005))
                || b.via_layers(v).iter().any(|layer| {
                    g.items.iter().any(|i| {
                        i.net != v.net && i.layers.contains(layer) && i.poly.intersects(&envelope)
                    }) || p.keepouts.iter().chain(&g.extra.via_keepouts).any(|r| {
                        (r.layer == *layer || r.layer == "*.Cu")
                            && polygon(&r.polygon).intersects(&envelope)
                    })
                })
            {
                continue;
            }
            // Preserve each old same-net neighbor connection, not just the far endpoint.
            if branches.iter().any(|(t, end, ti)| {
                let replacement = line(&[*end, q], t.width);
                g.graph
                    .neighbors(NodeIndex::new(*ti))
                    .any(|n| n.index() != vi && !replacement.intersects(&g.items[n.index()].poly))
            }) {
                continue;
            }
            best = Some(q);
            best_len = length;
        }
        if let Some(q) = best {
            emit(
                e,
                rule,
                vec![
                    v.id.clone(),
                    branches[0].0.id.clone(),
                    branches[1].0.id.clone(),
                ],
                v.at,
                "",
                format!(
                    "Move via to ({:.3}, {:.3}) mm and reconnect the two fixed endpoints: saves {:.3} mm without adding bends; clearance-screened, native DRC required",
                    q.x,
                    q.y,
                    original - best_len
                ),
                &[
                    ("candidate_x_mm", q.x),
                    ("candidate_y_mm", q.y),
                    ("original_mm", original),
                    ("candidate_mm", best_len),
                    ("saved_mm", original - best_len),
                ],
            );
        }
    }
}

pub fn route_chains(b: &Board, tol: f64) -> Vec<(Vec<String>, Vec<Point>)> {
    let mut ends = vec![Vec::<usize>::new(); b.tracks.len() * 2];
    for (i, a) in b.tracks.iter().enumerate() {
        for (j, z) in b.tracks.iter().enumerate().skip(i + 1) {
            if a.net != z.net || a.layer != z.layer {
                continue;
            }
            for (k, p) in [a.a, a.b].iter().enumerate() {
                for (l, q) in [z.a, z.b].iter().enumerate() {
                    if p.distance(*q) <= tol {
                        ends[2 * i + k].push(2 * j + l);
                        ends[2 * j + l].push(2 * i + k);
                    }
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut chains = vec![];
    for start in 0..ends.len() {
        if ends[start].len() == 1 || seen.contains(&(start / 2)) {
            continue;
        }
        let mut cur = start;
        let mut ids = vec![];
        let mut ps = vec![];
        loop {
            if !seen.insert(cur / 2) {
                break;
            }
            let t = &b.tracks[cur / 2];
            ids.push(t.id.clone());
            ps.push(if cur % 2 == 0 { t.a } else { t.b });
            let other = cur ^ 1;
            if ends[other].len() != 1
                || ends[ends[other][0]].len() != 1
                || seen.contains(&(ends[other][0] / 2))
            {
                ps.push(if other % 2 == 0 { t.a } else { t.b });
                break;
            }
            cur = ends[other][0];
        }
        if ps.len() == ids.len() + 1 {
            chains.push((ids, ps));
        }
    }
    chains
}
/// Infer small geometric groups, not electrical equivalence or move legality.
fn passive_alignment(e: &mut Emitter) {
    let rule = "placement.passive_alignment";
    ready(e, rule, true);
    let b = e.board;
    let intent = &e.config.intent;
    let policy = &intent.passive_alignment;
    type PassiveGroupKey = (String, String, char, i32);
    type PassiveMember<'a> = (&'a Part, Point);
    let mut buckets: BTreeMap<PassiveGroupKey, Vec<PassiveMember<'_>>> = BTreeMap::new();
    for f in &b.parts {
        let Some(kind) = f.reference.chars().next() else {
            continue;
        };
        if !matches!(kind, 'R' | 'C')
            || !f.reference[1..].chars().all(|c| c.is_ascii_digit())
            || f.footprint.is_empty()
            || policy.exclude_references.contains(&f.reference)
            || intent
                .loops
                .iter()
                .any(|l| l.capacitor == f.reference || l.power_pin.reference == f.reference)
        {
            continue;
        }
        let pads: Vec<_> = b
            .pads
            .iter()
            .filter(|p| p.reference == f.reference)
            .collect();
        if pads.len() != 2
            || pads.iter().any(|p| p.kind != "smd")
            || pads.iter().any(|p| {
                intent.route.tuned_nets.contains(&p.net)
                    || intent
                        .pairs
                        .iter()
                        .any(|q| q.positive == p.net || q.negative == p.net)
            })
        {
            continue;
        }
        let d = pads[1].at.minus(pads[0].at);
        if d.x.hypot(d.y) < 0.001 {
            continue;
        }
        let a = d.y.atan2(d.x).to_degrees().rem_euclid(180.);
        let orientation = if a.min(180. - a) <= policy.angle_tolerance_deg {
            0
        } else if (a - 90.).abs() <= policy.angle_tolerance_deg {
            90
        } else {
            continue;
        };
        let center = Point {
            x: (pads[0].at.x + pads[1].at.x) / 2.,
            y: (pads[0].at.y + pads[1].at.y) / 2.,
        };
        if intent.sensitive.iter().any(|r| {
            (r.layer == f.layer || r.layer == "*.Cu")
                && polygon(&r.polygon).contains(&geo::Point::new(center.x, center.y))
        }) {
            continue;
        }
        buckets
            .entry((f.footprint.clone(), f.layer.clone(), kind, orientation))
            .or_default()
            .push((f, center));
    }
    for members in buckets.values() {
        for axis in ["row", "column"] {
            let along = |p: Point| if axis == "row" { p.x } else { p.y };
            let across = |p: Point| if axis == "row" { p.y } else { p.x };
            let mut ordered = members.clone();
            ordered.sort_by(|a, b| {
                across(a.1)
                    .total_cmp(&across(b.1))
                    .then(a.0.id.cmp(&b.0.id))
            });
            let mut used = BTreeSet::new();
            for (_, seed) in &ordered {
                let mut band: Vec<_> = ordered
                    .iter()
                    .copied()
                    .filter(|(f, p)| {
                        !used.contains(&f.id)
                            && across(*p) >= across(*seed)
                            && across(*p) - across(*seed) <= policy.max_offset_mm
                    })
                    .collect();
                band.sort_by(|a, b| along(a.1).total_cmp(&along(b.1)).then(a.0.id.cmp(&b.0.id)));
                let mut start = 0;
                for end in 1..=band.len() {
                    if end < band.len()
                        && along(band[end].1) - along(band[end - 1].1) <= policy.max_gap_mm
                    {
                        continue;
                    }
                    let group = &band[start..end];
                    start = end;
                    if group.len() < policy.min_group {
                        continue;
                    }
                    let mut offsets: Vec<_> = group.iter().map(|(_, p)| across(*p)).collect();
                    offsets.sort_by(f64::total_cmp);
                    let target = offsets[offsets.len() / 2];
                    let max_move = offsets
                        .iter()
                        .map(|v| (v - target).abs())
                        .fold(0., f64::max);
                    if max_move <= policy.tolerance_mm {
                        continue;
                    }
                    let refs = group
                        .iter()
                        .map(|(f, _)| f.reference.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let moves = group
                        .iter()
                        .filter(|(_, p)| (across(*p) - target).abs() > policy.tolerance_mm)
                        .map(|(f, p)| format!("{} {:+.3} mm", f.reference, target - across(*p)))
                        .collect::<Vec<_>>()
                        .join("; ");
                    emit(
                        e,
                        rule,
                        group.iter().map(|(f, _)| f.id.clone()).collect(),
                        group[0].1,
                        axis,
                        format!(
                            "Possible passive {axis}: {refs}. Align pad centers to {}={target:.3} mm ({moves}). Geometric suggestion only: review function, routing, courtyards and locked placement before moving",
                            if axis == "row" { "y" } else { "x" }
                        ),
                        &[
                            ("target_coordinate_mm", target),
                            ("max_move_mm", max_move),
                            ("members", group.len() as f64),
                        ],
                    );
                    used.extend(group.iter().map(|(f, _)| f.id.clone()));
                }
            }
        }
    }
}

fn placement(e: &mut Emitter, g: &Geometry) {
    let b = e.board;
    let c = &e.config.intent;
    for id in [
        "placement.airwire_cost",
        "placement.rotation",
        "placement.congestion",
    ] {
        ready(e, id, !c.placement.is_empty());
    }
    for p in &c.placement {
        let Some(f) = part(b, &p.reference) else {
            contract(
                e,
                "placement.airwire_cost",
                &p.reference,
                "Placement contract component absent",
            );
            continue;
        };
        let own: Vec<_> = b
            .pads
            .iter()
            .filter(|a| a.reference == p.reference)
            .collect();
        let partners: Vec<_> = b
            .pads
            .iter()
            .filter(|a| p.partners.contains(&a.reference))
            .collect();
        let cost = |delta: Point, angle: f64| {
            own.iter()
                .map(|a| {
                    let pos = a.at.minus(f.at).rotate(angle).plus(f.at).plus(delta);
                    partners
                        .iter()
                        .filter(|z| z.net == a.net && !a.net.is_empty())
                        .map(|z| pos.distance(z.at))
                        .fold(0., f64::max)
                })
                .sum::<f64>()
        };
        let original = cost(Point { x: 0., y: 0. }, 0.);
        for d in [
            Point {
                x: p.step_mm,
                y: 0.,
            },
            Point {
                x: -p.step_mm,
                y: 0.,
            },
            Point {
                x: 0.,
                y: p.step_mm,
            },
            Point {
                x: 0.,
                y: -p.step_mm,
            },
        ] {
            let saving = original - cost(d, 0.);
            if saving > p.min_saving_mm {
                emit(
                    e,
                    "placement.airwire_cost",
                    vec![f.id.clone()],
                    f.at,
                    &format!("{},{}", d.x, d.y),
                    "Candidate translation reduces weighted endpoint distance; check bodies and placement legality",
                    &[("saving_mm", saving), ("dx_mm", d.x), ("dy_mm", d.y)],
                );
            }
        }
        for &r in &p.rotations_deg {
            let saving = original - cost(Point { x: 0., y: 0. }, r);
            if saving > p.min_saving_mm {
                emit(
                    e,
                    "placement.rotation",
                    vec![f.id.clone()],
                    f.at,
                    &r.to_string(),
                    "Allowed candidate rotation reduces endpoint distance",
                    &[("saving_mm", saving), ("rotation_deg", r)],
                );
            }
        }
        let near = b
            .tracks
            .iter()
            .filter(|t| line_distance(f.at, t.a, t.b) < p.radius_mm)
            .count();
        if near > p.max_connections {
            emit(
                e,
                "placement.congestion",
                vec![f.id.clone()],
                f.at,
                "",
                "Local trace demand exceeds the explicit congestion budget",
                &[("segments", near as f64)],
            );
        }
    }
    for id in ["placement.pitch", "placement.orientation"] {
        ready(e, id, !c.peers.is_empty());
    }
    for p in &c.peers {
        let parts: Vec<_> = p.references.iter().filter_map(|r| part(b, r)).collect();
        if parts.len() != p.references.len() {
            contract(
                e,
                "placement.pitch",
                &p.name,
                "Peer group component missing",
            );
            continue;
        }
        let mut pos: Vec<_> = parts
            .iter()
            .map(|r| if p.axis == "row" { r.at.x } else { r.at.y })
            .collect();
        pos.sort_by(f64::total_cmp);
        if pos
            .windows(2)
            .any(|w| (w[1] - w[0] - p.pitch_mm).abs() > p.tolerance_mm)
        {
            emit(
                e,
                "placement.pitch",
                parts.iter().map(|r| r.id.clone()).collect(),
                parts[0].at,
                &p.name,
                "Peer spacing differs from specified pitch",
                &[],
            );
        }
        for f in parts {
            if angle(f.angle, p.angle_deg) > p.angle_tolerance_deg {
                emit(
                    e,
                    "placement.orientation",
                    vec![f.id.clone()],
                    f.at,
                    &p.name,
                    "Peer orientation differs from the specified orientation",
                    &[],
                );
            }
        }
    }
    for id in ["channel.topology", "channel.geometry", "channel.polarity"] {
        ready(e, id, !c.channels.is_empty());
    }
    for ch in &c.channels {
        for pair in &ch.pairs {
            let (Some(a), Some(z)) = (part(b, &pair[0]), part(b, &pair[1])) else {
                contract(
                    e,
                    "channel.topology",
                    &ch.name,
                    "Mapped channel component missing",
                );
                continue;
            };
            let mut left: BTreeMap<_, _> = b
                .pads
                .iter()
                .filter(|p| p.reference == pair[0])
                .map(|p| {
                    (
                        p.number.clone(),
                        ch.net_map.get(&p.net).unwrap_or(&p.net).clone(),
                    )
                })
                .collect();
            left.retain(|_, v| !v.is_empty());
            let mut right: BTreeMap<_, _> = b
                .pads
                .iter()
                .filter(|p| p.reference == pair[1])
                .map(|p| (p.number.clone(), p.net.clone()))
                .collect();
            right.retain(|_, v| !v.is_empty());
            let ids = || vec![a.id.clone(), z.id.clone()];
            if left != right {
                emit(
                    e,
                    "channel.topology",
                    ids(),
                    a.at,
                    &ch.name,
                    "Mapped repeated-channel pad/net sets differ",
                    &[],
                );
            }
            let expected = a.at.rotate(ch.rotation_deg).plus(ch.translate);
            if expected.distance(z.at) > ch.tolerance_mm
                || angle(a.angle + ch.rotation_deg, z.angle) > 1.
            {
                emit(
                    e,
                    "channel.geometry",
                    ids(),
                    a.at,
                    &ch.name,
                    "Repeated-channel placement differs from declared transform",
                    &[("position_error_mm", expected.distance(z.at))],
                );
            }
            if ch.polarized.contains(&pair[0]) && angle(a.angle + ch.rotation_deg, z.angle) > 1. {
                emit(
                    e,
                    "channel.polarity",
                    ids(),
                    a.at,
                    &ch.name,
                    "Polarized peer orientation differs; inspect anode/cathode or positive pad mapping",
                    &[],
                );
            }
        }
    }
    ready(e, "block.flow", !c.flows.is_empty());
    for flow in &c.flows {
        for p in flow.references.windows(2) {
            if let (Some(a), Some(z)) = (part(b, &p[0]), part(b, &p[1])) {
                let d = if flow.axis == "x" {
                    z.at.x - a.at.x
                } else {
                    z.at.y - a.at.y
                };
                if d < flow.minimum_gap_mm {
                    emit(
                        e,
                        "block.flow",
                        vec![a.id.clone(), z.id.clone()],
                        a.at,
                        &flow.name,
                        "Stages violate declared direction/minimum spacing",
                        &[("gap_mm", d)],
                    );
                }
            } else {
                contract(e, "block.flow", &flow.name, "Flow stage missing");
            }
        }
    }
    ready(e, "power.corridor", !c.corridors.is_empty());
    for co in &c.corridors {
        if !co.compatible {
            continue;
        }
        let centers: Vec<_> = co
            .nets
            .iter()
            .filter_map(|net| {
                let ps: Vec<_> = b
                    .tracks
                    .iter()
                    .filter(|t| &t.net == net)
                    .flat_map(|t| [t.a, t.b])
                    .collect();
                (!ps.is_empty()).then(|| Point {
                    x: ps.iter().map(|p| p.x).sum::<f64>() / ps.len() as f64,
                    y: ps.iter().map(|p| p.y).sum::<f64>() / ps.len() as f64,
                })
            })
            .collect();
        if centers.len() != co.nets.len() {
            contract(
                e,
                "power.corridor",
                &co.name,
                "Expected power corridor net lacks routes",
            );
        } else if centers.iter().any(|a| {
            centers
                .iter()
                .any(|z| a.distance(*z) > co.max_separation_mm)
        }) {
            let ids = b
                .tracks
                .iter()
                .filter(|t| co.nets.contains(&t.net))
                .map(|t| t.id.clone())
                .collect();
            emit(
                e,
                "power.corridor",
                ids,
                centers[0],
                &co.name,
                "Compatible power-net routing centroids are separated; inspect shared corridor opportunity",
                &[],
            );
        }
    }
    ready(e, "noise.partition", !c.sensitive.is_empty());
    for r in &c.sensitive {
        for t in &b.tracks {
            if (r.layer == "*.Cu" || r.layer == t.layer)
                && !r.nets.contains(&t.net)
                && polygon(&r.polygon).intersects(&line(&[t.a, t.b], t.width))
            {
                emit(
                    e,
                    "noise.partition",
                    vec![t.id.clone()],
                    t.a,
                    &r.id,
                    "Foreign net crosses declared sensitive region",
                    &[],
                );
            }
        }
    }
    let _ = g;
}
