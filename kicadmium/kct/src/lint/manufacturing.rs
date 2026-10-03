//! Assembly, release and independent-check contracts; no generated pass assertions.
use crate::lint::{advanced::*, copper::*, design::canonical, model::*, Emitter};
use geo::{Area, BooleanOps, Buffer, Contains, Intersects, MultiPolygon};
use std::collections::BTreeSet;
pub fn run(e: &mut Emitter, g: &Geometry) {
    let b = e.board;
    let c = &e.config.intent;
    for id in [
        "testpoint.coverage",
        "testpoint.access",
        "testpoint.ground",
        "testpoint.label",
    ] {
        ready(e, id, !c.probes.is_empty());
    }
    for p in &c.probes {
        let candidates: Vec<_> = b
            .pads
            .iter()
            .filter(|x| x.net == p.net && p.reference.as_ref().is_some_and(|r| &x.reference == r))
            .collect();
        if candidates.is_empty() {
            contract(
                e,
                "testpoint.coverage",
                &p.net,
                "Declared probe location has no pad on the required net",
            );
            continue;
        }
        for pad in candidates {
            let side = if on_layer(&pad.layers, "F.Cu") {
                "F"
            } else {
                "B"
            };
            if c.bodies.is_empty() {
                ready(e, "testpoint.access", false);
            } else {
                for body in &c.bodies {
                    if body.id != pad.reference
                        && body.side == side
                        && body.z_max_mm > 0.
                        && rect(&body.bounds).intersects(&disk(pad.at, p.radius_mm))
                    {
                        emit(
                            e,
                            "testpoint.access",
                            vec![pad.id.clone()],
                            pad.at,
                            &body.id,
                            "Probe envelope overlaps a declared component body",
                            &[],
                        );
                    }
                }
            }
            let d = b
                .pads
                .iter()
                .filter(|x| x.net == p.ground_net && on_layer(&x.layers, &format!("{side}.Cu")))
                .map(|x| x.at.distance(pad.at))
                .fold(f64::INFINITY, f64::min);
            if d > p.max_ground_mm {
                emit(
                    e,
                    "testpoint.ground",
                    vec![pad.id.clone()],
                    pad.at,
                    &p.net,
                    "No nearby same-side ground pad for probing",
                    &[("distance_mm", d.min(1e9))],
                );
            }
            if !g.extra.texts.iter().any(|t| {
                t.text == p.expected_label
                    && t.layer.starts_with(side)
                    && t.at.distance(pad.at) <= p.max_ground_mm
            }) {
                emit(
                    e,
                    "testpoint.label",
                    vec![pad.id.clone()],
                    pad.at,
                    &p.net,
                    "Expected probe label absent near pad on accessible side",
                    &[],
                );
            }
        }
    }
    for id in ["pair.skew", "pair.spacing"] {
        ready(e, id, !c.pairs.is_empty());
    }
    for p in &c.pairs {
        let a = route_length(b, &g.extra, &p.positive);
        let z = route_length(b, &g.extra, &p.negative);
        let ts: Vec<_> = b
            .tracks
            .iter()
            .filter(|t| t.net == p.positive || t.net == p.negative)
            .collect();
        let ids = || ts.iter().map(|t| t.id.clone()).collect();
        if a == 0. || z == 0. {
            contract(e, "pair.skew", &p.name, "Pair member lacks routed copper");
        } else if (a - z).abs() > p.max_skew_mm || a.max(z) > p.max_length_mm {
            emit(
                e,
                "pair.skew",
                ids(),
                ts.first().map_or(Point { x: 0., y: 0. }, |t| t.a),
                &p.name,
                "Pair total copper-length mismatch or length budget exceeded; branched nets require path extraction",
                &[
                    ("positive_mm", a),
                    ("negative_mm", z),
                    ("skew_mm", (a - z).abs()),
                ],
            );
        }
        let mut compared = false;
        for t in ts.iter().filter(|t| t.net == p.positive) {
            let d = t.b.minus(t.a);
            let len = t.a.distance(t.b);
            if len < 1e-9 {
                continue;
            }
            let mut best: Option<(f64, &Track)> = None;
            for q in ts.iter().filter(|t| t.net == p.negative) {
                let v = q.b.minus(q.a);
                if t.layer != q.layer || (d.x * v.y - d.y * v.x).abs() > 1e-6 {
                    continue;
                }
                let proj = |r: Point| {
                    let r = r.minus(t.a);
                    (r.x * d.x + r.y * d.y) / len
                };
                if proj(q.a).max(proj(q.b)) <= 0. || proj(q.a).min(proj(q.b)) >= len {
                    continue;
                }
                let w = q.a.minus(t.a);
                let gap = (w.x * d.y - w.y * d.x).abs() / len - (t.width + q.width) / 2.;
                if best.is_none_or(|(old, _)| gap < old) {
                    best = Some((gap, q));
                }
            }
            if let Some((gap, q)) = best {
                compared = true;
                if (gap - p.gap_mm).abs() > p.gap_tolerance_mm {
                    emit(
                        e,
                        "pair.spacing",
                        vec![t.id.clone(), q.id.clone()],
                        t.a,
                        &p.name,
                        "Parallel pair edge gap differs from specified gap",
                        &[("gap_mm", gap)],
                    );
                }
            }
        }
        if !compared {
            e.coverage
                .insert("pair.spacing".into(), "unsupported_geometry".into());
        }
    }
    ready(e, "reference.discontinuity", !c.references.is_empty());
    for r in &c.references {
        let fills: Vec<_> = g
            .extra
            .zones
            .iter()
            .filter(|z| z.net == r.plane_net && z.layer == r.plane_layer)
            .flat_map(|z| &z.filled)
            .collect();
        if fills.is_empty() {
            ready(e, "reference.discontinuity", false);
            continue;
        }
        let plane = fills.iter().fold(MultiPolygon(vec![]), |a, p| {
            a.union(&MultiPolygon(vec![polygon(p)]))
        });
        for t in b
            .tracks
            .iter()
            .filter(|t| t.net == r.net && t.layer == r.layer)
        {
            let exposed = line(&[t.a, t.b], t.width + 2. * r.margin_mm)
                .difference(&plane)
                .unsigned_area();
            if exposed > 1e-5 {
                emit(
                    e,
                    "reference.discontinuity",
                    vec![t.id.clone()],
                    t.a,
                    &r.plane_net,
                    "Signal corridor crosses a gap in supplied reference-plane fill",
                    &[("uncovered_mm2", exposed)],
                );
            }
        }
    }
    ready(e, "rf.launch", !c.launches.is_empty());
    for l in &c.launches {
        let Some(p) = b
            .pads
            .iter()
            .find(|p| p.reference == l.reference && p.number == l.signal_pad)
        else {
            contract(e, "rf.launch", &l.reference, "Launch signal pad missing");
            continue;
        };
        let count = b
            .vias
            .iter()
            .filter(|v| {
                v.net == l.ground_net
                    && v.at.distance(p.at) <= l.radius_mm
                    && b.via_layers(v).iter().any(|l| on_layer(&p.layers, l))
            })
            .count();
        let attached: Vec<_> = b
            .tracks
            .iter()
            .filter(|t| {
                t.net == p.net
                    && on_layer(&p.layers, &t.layer)
                    && (pad_distance(t.a, p) <= 0. || pad_distance(t.b, p) <= 0.)
            })
            .collect();
        if count < l.minimum_vias
            || attached.is_empty()
            || attached
                .iter()
                .any(|t| t.width < l.min_width_mm || t.width > l.max_width_mm)
        {
            emit(
                e,
                "rf.launch",
                vec![p.id.clone()],
                p.at,
                &l.reference,
                "Launch grounding-via count or attached trace width violates explicit launch specification",
                &[("ground_vias", count as f64)],
            );
        }
    }
    for id in ["capacitor.role", "capacitor.rating"] {
        ready(e, id, !c.capacitors.is_empty());
    }
    for cap in &c.capacitors {
        let Some(f) = part(b, &cap.reference) else {
            contract(
                e,
                "capacitor.role",
                &cap.reference,
                "Specified capacitor missing",
            );
            continue;
        };
        if cap.role.is_empty()
            || cap.source.is_empty()
            || !b
                .pads
                .iter()
                .any(|p| p.reference == cap.reference && p.net == cap.net)
        {
            emit(
                e,
                "capacitor.role",
                vec![f.id.clone()],
                f.at,
                &cap.reference,
                "Capacitor role/provenance or required net connection is missing",
                &[],
            );
        }
        if cap.max_volts > cap.rating_volts * cap.derating_fraction
            || cap.effective_uf < cap.required_uf
        {
            emit(
                e,
                "capacitor.rating",
                vec![f.id.clone()],
                f.at,
                &cap.reference,
                "Capacitor voltage headroom or effective capacitance below explicit requirement",
                &[
                    ("usable_volts", cap.rating_volts * cap.derating_fraction),
                    ("effective_uf", cap.effective_uf),
                ],
            );
        }
    }
    ready(e, "parts.package", !c.packages.is_empty());
    for p in &c.packages {
        let Some(f) = part(b, &p.reference) else {
            contract(e, "parts.package", &p.reference, "Required package missing");
            continue;
        };
        let actual: BTreeSet<_> = b
            .pads
            .iter()
            .filter(|x| x.reference == p.reference)
            .map(|x| x.number.clone())
            .collect();
        let expected: BTreeSet<_> = p.pads.iter().cloned().collect();
        let props = g.extra.fields.get(&p.reference);
        let matches =
            |name: &str, value: &str| props.and_then(|x| x.get(name)).is_some_and(|x| x == value);
        if f.footprint != p.footprint
            || actual != expected
            || p.source.is_empty()
            || !matches("Manufacturer", &p.manufacturer)
            || !matches("MPN", &p.mpn)
        {
            emit(
                e,
                "parts.package",
                vec![f.id.clone()],
                f.at,
                &p.reference,
                "Footprint, pad set or manufacturer fields disagree with approved package contract",
                &[],
            );
        }
    }
    if ready(e, "mechanics.edge_clearance", g.outline.is_some()) {
        let outline = g.outline.as_ref().unwrap();
        if g.uncertain {
            e.coverage.insert(
                "mechanics.edge_clearance".into(),
                "unsupported_geometry".into(),
            );
        }
        for o in &g.items {
            if !outline.contains(&o.poly.buffer(c.route.edge_clearance_mm)) {
                emit(
                    e,
                    "mechanics.edge_clearance",
                    vec![o.id.clone()],
                    o.at,
                    "",
                    "Copper envelope violates configured board-edge or cutout margin",
                    &[],
                );
            }
        }
    }
    ready(
        e,
        "mechanics.access",
        !c.access.is_empty() && !c.bodies.is_empty(),
    );
    for a in &c.access {
        for body in &c.bodies {
            if body.id != a.reference && body.side == a.side && overlaps(&a.envelope, &body.bounds)
            {
                let ids = part(b, &a.reference)
                    .map(|p| vec![p.id.clone()])
                    .unwrap_or_else(|| vec![format!("contract:{}", a.reference)]);
                emit(
                    e,
                    "mechanics.access",
                    ids,
                    a.envelope.min,
                    &body.id,
                    "Declared insertion/tool-access envelope intersects component body",
                    &[],
                );
            }
        }
    }
    ready(e, "thermal.spokes", !c.thermals.is_empty());
    for t in &c.thermals {
        let Some(p) = b
            .pads
            .iter()
            .find(|p| p.reference == t.reference && p.number == t.pad)
        else {
            contract(e, "thermal.spokes", &t.reference, "Thermal pad absent");
            continue;
        };
        let zones: Vec<_> = g
            .extra
            .zones
            .iter()
            .filter(|z| {
                z.net == p.net
                    && on_layer(&p.layers, &z.layer)
                    && z.outlines
                        .iter()
                        .any(|r| polygon(r).intersects(&pad_poly(p)))
            })
            .collect();
        if zones.is_empty() {
            ready(e, "thermal.spokes", false);
            continue;
        }
        for z in zones {
            let connection = g.extra.pad_thermals.get(&p.id).unwrap_or(&z.connection);
            if connection != &t.allowed_connection
                || z.gap > t.max_gap_mm
                || z.spoke < t.min_spoke_mm
            {
                emit(
                    e,
                    "thermal.spokes",
                    vec![p.id.clone()],
                    p.at,
                    &z.layer,
                    "Zone connection mode, thermal gap or spoke width violates contract",
                    &[("gap_mm", z.gap), ("spoke_width_mm", z.spoke)],
                );
            }
            if z.filled.is_empty() {
                ready(e, "thermal.spokes", false);
                continue;
            }
            if connection == "thermal" {
                let fill = z.filled.iter().fold(MultiPolygon(vec![]), |a, q| {
                    a.union(&MultiPolygon(vec![polygon(q)]))
                });
                let radius = p.size.x.max(p.size.y) / 2. + z.gap / 2.;
                let mask: Vec<_> = (0..720)
                    .map(|i| {
                        let a = i as f64 * std::f64::consts::TAU / 720.;
                        fill.contains(&geo::Point::new(
                            p.at.x + radius * a.cos(),
                            p.at.y + radius * a.sin(),
                        ))
                    })
                    .collect();
                let count = (0..720)
                    .filter(|&i| mask[i] && !mask[(i + 719) % 720])
                    .count();
                if count < t.min_spokes {
                    emit(
                        e,
                        "thermal.spokes",
                        vec![p.id.clone()],
                        p.at,
                        &format!("{}:fill", z.layer),
                        "Sampled thermal ring has fewer copper crossings than required; inspect polygon approximation",
                        &[("sampled_spokes", count as f64)],
                    );
                }
            }
        }
    }
    for id in ["bom.completeness", "cpl.orientation"] {
        ready(e, id, c.assembly.is_some());
    }
    if let Some(a) = &c.assembly {
        for f in &b.parts {
            if a.excluded.contains(&f.reference) {
                continue;
            }
            let fields = g.extra.fields.get(&f.reference);
            for name in &a.fields {
                if fields
                    .and_then(|m| m.get(name))
                    .is_none_or(|v| v.trim().is_empty() || v == "~")
                {
                    emit(
                        e,
                        "bom.completeness",
                        vec![f.id.clone()],
                        f.at,
                        name,
                        format!("Assembly BOM missing {name}"),
                        &[],
                    );
                }
            }
            let row = a.placements.iter().find(|p| p.reference == f.reference);
            let valid = row.is_some_and(|p| {
                p.at.plus(a.origin).distance(f.at) <= a.tolerance_mm
                    && angle(
                        p.angle_deg + a.angle_offsets.get(&f.reference).copied().unwrap_or(0.),
                        f.angle,
                    ) <= a.tolerance_deg
                    && p.side == if f.layer.starts_with('F') { "F" } else { "B" }
            });
            if !valid {
                emit(
                    e,
                    "cpl.orientation",
                    vec![f.id.clone()],
                    f.at,
                    &f.reference,
                    "Placement output is missing or differs in position, side or specified rotation convention",
                    &[],
                );
            }
        }
        for p in &a.placements {
            if part(b, &p.reference).is_none() {
                contract(
                    e,
                    "cpl.orientation",
                    &p.reference,
                    "Placement output contains unknown reference",
                );
            }
        }
    }
    ready(e, "silk.collisions", true);
    for t in &g.extra.texts {
        let side = t.layer.chars().next().unwrap_or('F');
        for p in &b.pads {
            if on_layer(&p.layers, &format!("{side}.Cu"))
                && rect(&t.bounds).intersects(&pad_poly(p))
            {
                emit(
                    e,
                    "silk.collisions",
                    vec![t.id.clone(), p.id.clone()],
                    t.at,
                    "",
                    "Approximate silkscreen text box overlaps pad copper; verify font and mask opening in KiCad",
                    &[],
                );
            }
        }
    }
    ready(e, "silk.content", !c.labels.is_empty());
    for l in &c.labels {
        let Some(f) = part(b, &l.reference) else {
            contract(e, "silk.content", &l.reference, "Label component absent");
            continue;
        };
        if !g.extra.texts.iter().any(|t| {
            t.text == l.text && t.layer == l.layer && t.at.distance(f.at) <= l.max_distance_mm
        }) {
            emit(
                e,
                "silk.content",
                vec![f.id.clone()],
                f.at,
                &l.text,
                "Expected same-layer nearby label missing",
                &[],
            );
        }
    }
    ready(e, "export.freshness", !c.artifacts.is_empty());
    for a in &c.artifacts {
        if a.expected_sha256.is_empty()
            || a.expected_sha256 != a.actual_sha256
            || a.built_from_sha256 != e.source_sha256
        {
            contract(
                e,
                "export.freshness",
                &a.name,
                "Artifact checksum or source revision differs from release expectation",
            );
        }
    }
    ready(e, "release.manifest", c.release.is_some());
    if let Some(r) = &c.release {
        for name in &r.required_artifacts {
            if !c.artifacts.iter().any(|a| {
                &a.name == name
                    && a.expected_sha256 == a.actual_sha256
                    && !a.actual_sha256.is_empty()
                    && a.built_from_sha256 == e.source_sha256
            }) {
                contract(
                    e,
                    "release.manifest",
                    name,
                    "Required current release artifact absent or stale",
                );
            }
        }
        for name in &r.required_checks {
            if !r.checks.iter().any(|x| {
                &x.tool == name
                    && !x.version.is_empty()
                    && x.source_sha256 == e.source_sha256
                    && x.unwaived_errors == 0
            }) {
                contract(
                    e,
                    "release.manifest",
                    name,
                    "Required native check is absent, stale, unversioned or has unwaived errors",
                );
            }
        }
    }
    for id in ["checker.transform", "checker.differential"] {
        ready(e, id, c.native.is_some());
    }
    if let Some(n) = &c.native {
        if n.source_sha256 != e.source_sha256 {
            for id in ["checker.transform", "checker.differential"] {
                e.coverage.insert(id.into(), "stale_input".into());
            }
        } else {
            for p in &b.pads {
                if n.pads
                    .get(&p.id)
                    .is_none_or(|q| q.distance(p.at) > n.tolerance_mm)
                {
                    emit(
                        e,
                        "checker.transform",
                        vec![p.id.clone()],
                        p.at,
                        "",
                        "Native parser pad coordinate differs or is missing",
                        &[],
                    );
                }
            }
            for id in n.pads.keys() {
                if !b.pads.iter().any(|p| &p.id == id) {
                    contract(
                        e,
                        "checker.transform",
                        id,
                        "Native parser reports an extra pad",
                    );
                }
            }
            if g.uncertain {
                e.coverage
                    .insert("checker.differential".into(), "unsupported_geometry".into());
            } else if canonical(&g.components(None)) != canonical(&n.connectivity) {
                contract(
                    e,
                    "checker.differential",
                    "connectivity",
                    "Native and lint connectivity equivalence classes disagree",
                );
            }
        }
    }
    ready(e, "checker.noise", !c.observations.is_empty());
    let rules: BTreeSet<_> = c.observations.iter().map(|o| &o.rule).collect();
    for rule in rules {
        let rows: Vec<_> = c.observations.iter().filter(|o| &o.rule == rule).collect();
        let negatives = rows.iter().filter(|o| !o.actual).count();
        let positives = rows.len() - negatives;
        let fp = rows.iter().filter(|o| !o.actual && o.predicted).count();
        let missed = rows.iter().filter(|o| o.actual && !o.predicted).count();
        let fpr = fp as f64 / negatives.max(1) as f64;
        let fnr = missed as f64 / positives.max(1) as f64;
        let mut keys = BTreeSet::new();
        if rows.iter().any(|o| {
            !keys.insert(&o.key)
                || fpr > o.max_false_positive_rate
                || fnr > o.max_false_negative_rate
        }) {
            contract(
                e,
                "checker.noise",
                rule,
                format!(
                    "Labeled evaluation exceeds error budget or duplicates observations: FP {fp}/{negatives}, FN {missed}/{positives}"
                ),
            );
        }
    }
    ready(e, "review.reattach", c.baseline.is_some());
    if let Some(base) = &c.baseline {
        for old in &base.objects {
            if old.finding_key.is_some() && !g.items.iter().any(|o| o.id == old.id) {
                for new in g.items.iter().filter(|o| {
                    o.net == old.net && o.kind == old.kind && o.at.distance(old.at) <= 0.5
                }) {
                    emit(
                        e,
                        "review.reattach",
                        vec![new.id.clone()],
                        new.at,
                        &old.id,
                        "Possible replacement for a reviewed deleted object; review again, suppression is never transferred automatically",
                        &[("distance_mm", new.at.distance(old.at))],
                    );
                }
            }
        }
    }
    ready(e, "annotation.stale", !c.annotations.is_empty());
    for a in &c.annotations {
        if a.source_sha256 != e.source_sha256
            || a.subjects.iter().any(|id| {
                !g.items.iter().any(|o| &o.id == id) && !b.parts.iter().any(|p| &p.id == id)
            })
        {
            contract(
                e,
                "annotation.stale",
                &a.id,
                "Annotation revision or referenced objects no longer match this board",
            );
        }
    }
}
