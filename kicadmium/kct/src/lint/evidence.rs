//! Evidence hashes bind a review decision to the facts a finding depends on.
//!
//! Engine 3 scopes evidence per rule so that unrelated, distant edits leave
//! reviews intact while nearby or rule-relevant edits reopen them:
//!
//! * **Subjects** are always hashed in full, with their footprint fields,
//!   pad thermal settings and footprint-owned unmodeled copper.
//! * **Local** rules (default) add every object within [`NEAR_MM`] of the
//!   finding's location or of any subject's geometry, regardless of net, plus
//!   zones that are nearby or share one of the finding's nets.
//! * **Whole-net** rules ([`WHOLE_NET`]), whose verdict depends on net
//!   topology or total length, also add all copper on the finding's nets.
//! * **Metadata** rules ([`METADATA`]) hash only their subjects and fields.
//! * **Config**: only the config fields the rule reads ([`config_paths`]);
//!   unmapped rules hash the whole config.
//!
//! Upgrading from engine 2 changes every evidence hash once: existing
//! exceptions read as `changed` and need one re-review (re-waive/re-flag).
use crate::lint::{
    copper::Extra,
    hash,
    model::{line_distance, Board, Point},
    Config, Finding,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: &str = "engine-3";
/// Neighbourhood radius around the finding location and subject geometry.
pub const NEAR_MM: f64 = 5.;

/// Rules whose verdict depends on whole-net topology, length or attachment.
pub const WHOLE_NET: &[&str] = &[
    "copper.island",
    "copper.stale_stub",
    "via.avoidable",
    "via.role_missing",
    "via.low_attachment",
    "via.return_distance",
    "route.detour",
    "route.tuning_integrity",
    "connectivity.regression",
    "power.width_capacity",
    "power.corridor",
    "pair.skew",
    "pair.spacing",
    "reference.discontinuity",
    "placement.airwire_cost",
    "placement.rotation",
    "placement.congestion",
    "contract.missing_net",
    "contract.net_width",
    "contract.net_layer",
    "contract.via_budget",
    "decoupling.pin_distance",
    "decoupling.loop_area",
    "regulator.feedback",
    "regulator.hot_loop",
    "pin.swap_candidate",
    "pin.swap_legality",
];

/// Rules about part metadata, schematic or release artifacts, not copper.
pub const METADATA: &[&str] = &[
    "bom.completeness",
    "cpl.orientation",
    "parts.package",
    "capacitor.role",
    "capacitor.rating",
    "feature.unrequested",
    "net.naming",
    "export.freshness",
    "release.manifest",
    "annotation.stale",
    "review.reattach",
    "checker.transform",
    "checker.differential",
    "checker.noise",
    "schematic.collisions",
    "schematic.junction",
    "schematic.stem",
    "schematic.reading_order",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Metadata,
    Local,
    WholeNet,
}
pub fn scope(rule: &str) -> Scope {
    if METADATA.contains(&rule) {
        Scope::Metadata
    } else if WHOLE_NET.contains(&rule) {
        Scope::WholeNet
    } else {
        Scope::Local
    }
}

const JOIN: &str = "/join_tolerance_mm";
const ANGLE: &str = "/angle_tolerance_deg";
const SHORT: &str = "/short_segment_mm";
/// Shared inputs of the advanced engine: route policy builds the geometry model.
const ADVANCED: [&str; 3] = [JOIN, "/intent/route", "/intent/roles"];

/// JSON pointers into `Config` that a rule reads; `None` hashes the whole config.
pub fn config_paths(rule: &str) -> Option<Vec<&'static str>> {
    let core: &[&str] = match rule {
        "trace.zero_length" | "trace.open_end" | "trace.duplicate" | "via.pad_overlap" => &[JOIN],
        "trace.short_segment" => &[JOIN, SHORT],
        "trace.off_angle" | "trace.mergeable" | "trace.acute_junction" => &[JOIN, ANGLE],
        "pad.oblique_exit" => &[JOIN, SHORT, ANGLE],
        "trace.minimum_width" => &["/min_trace_mm"],
        "trace.no_net" | "via.no_net" => &[],
        "via.annulus" => &["/min_annulus_mm"],
        "via.drill" => &["/min_drill_mm"],
        "via.cluster" => &["/via_cluster_mm"],
        "route.detour" => &[JOIN, "/detour_ratio", "/detour_excess_mm"],
        "route.bend_count" => &[JOIN, ANGLE, "/bend_threshold"],
        "contract.missing_net"
        | "contract.net_width"
        | "contract.net_layer"
        | "contract.via_budget" => &[JOIN, "/net_rules"],
        "contract.pin_net" => &["/pin_nets"],
        "contract.proximity" | "contract.side" => &["/proximity"],
        "contract.alignment" => &["/groups"],
        // Emitted by both engines.
        "trace.width_transition" => &[JOIN, ANGLE, "/intent/route", "/intent/roles"],
        "route.width_island" => &[JOIN, "/narrow_run_mm", "/intent/route", "/intent/roles"],
        "route.layer_excursion" => &[JOIN, "/intent/route", "/intent/roles"],
        _ => {
            let intent: &[&str] = match rule {
                "copper.island"
                | "copper.stale_stub"
                | "copper.duplicate_overlap"
                | "via.avoidable"
                | "via.role_missing"
                | "via.low_attachment"
                | "via.overshoot"
                | "route.legal_shortcut"
                | "route.backtrack"
                | "pad.grazing"
                | "neck.clearance_reason"
                | "width.taper"
                | "mechanics.edge_clearance"
                | "silk.collisions" => &[],
                "route.tuning_integrity" | "connectivity.regression" | "review.reattach" => {
                    &["/intent/baseline"]
                }
                "via.return_distance" | "reference.discontinuity" => &["/intent/references"],
                "placement.passive_alignment" => {
                    &["/intent/passive_alignment", "/intent/sensitive"]
                }
                "placement.airwire_cost" | "placement.rotation" | "placement.congestion" => {
                    &["/intent/placement"]
                }
                "placement.pitch" | "placement.orientation" => &["/intent/peers"],
                "channel.topology" | "channel.geometry" | "channel.polarity" => {
                    &["/intent/channels"]
                }
                "block.flow" => &["/intent/flows"],
                "power.corridor" => &["/intent/corridors"],
                "noise.partition" => &["/intent/sensitive"],
                "decoupling.pin_distance"
                | "decoupling.loop_area"
                | "regulator.feedback"
                | "regulator.hot_loop" => &["/intent/loops"],
                "schematic.collisions"
                | "schematic.junction"
                | "schematic.stem"
                | "schematic.reading_order" => &["/intent/schematic"],
                "netlist.parity" => &["/intent/netlist"],
                "power.width_capacity" => &["/intent/currents"],
                "feature.unrequested" => &["/intent/features"],
                "pin.swap_candidate" | "pin.swap_legality" => &["/intent/swaps"],
                "net.naming" => &["/intent/naming"],
                "power.pin_roles" => &["/intent/pin_roles"],
                "strap.truth_table" => &["/intent/straps"],
                "interface.completeness" => &["/intent/interfaces"],
                "connector.mating" => &["/intent/mating"],
                "source.series_parallel" | "source.backfeed" | "source.polarity" => {
                    &["/intent/modes"]
                }
                "testpoint.coverage" | "testpoint.access" | "testpoint.ground"
                | "testpoint.label" => &["/intent/probes", "/intent/bodies"],
                "pair.skew" | "pair.spacing" => &["/intent/pairs"],
                "rf.launch" => &["/intent/launches"],
                "capacitor.role" | "capacitor.rating" => &["/intent/capacitors"],
                "parts.package" => &["/intent/packages"],
                "mechanics.access" => &["/intent/access", "/intent/bodies"],
                "thermal.spokes" => &["/intent/thermals"],
                "bom.completeness" | "cpl.orientation" => &["/intent/assembly"],
                "silk.content" => &["/intent/labels"],
                "export.freshness" => &["/intent/artifacts"],
                "release.manifest" => &["/intent/release", "/intent/artifacts"],
                "route.redundant_bend" => &[SHORT, ANGLE],
                "checker.transform" | "checker.differential" => &["/intent/native"],
                "checker.noise" => &["/intent/observations"],
                "annotation.stale" => &["/intent/annotations"],
                _ => return None,
            };
            let mut v: Vec<&str> = intent.to_vec();
            if scope(rule) != Scope::Metadata {
                v.extend(ADVANCED);
            }
            return Some(v);
        }
    };
    Some(core.to_vec())
}

/// The config slice hashed into a rule's evidence.
pub fn config_slice(rule: &str, config: &Config) -> Value {
    let full = serde_json::to_value(config).unwrap_or(Value::Null);
    match config_paths(rule) {
        None => full,
        Some(paths) => Value::Object(
            paths
                .into_iter()
                .map(|p| {
                    (
                        p.to_owned(),
                        full.pointer(p).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect(),
        ),
    }
}

/// A neighbourhood segment with a radius (degenerate for points).
struct Anchor {
    a: Point,
    b: Point,
    r: f64,
}
fn orient(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}
fn seg_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    if o1 * o2 < 0. && o3 * o4 < 0. {
        return 0.;
    }
    line_distance(c, a, b)
        .min(line_distance(d, a, b))
        .min(line_distance(a, c, d))
        .min(line_distance(b, c, d))
}
struct Hood(Vec<Anchor>);
impl Hood {
    fn seg(&self, a: Point, b: Point, r: f64) -> bool {
        self.0
            .iter()
            .any(|h| seg_distance(h.a, h.b, a, b) - h.r - r < NEAR_MM)
    }
    fn point(&self, p: Point, r: f64) -> bool {
        self.seg(p, p, r)
    }
    fn polyline(&self, pts: &[Point], r: f64) -> bool {
        match pts {
            [] => true, // unknown extent: include conservatively
            [p] => self.point(*p, r),
            _ => pts.windows(2).any(|w| self.seg(w[0], w[1], r)),
        }
    }
    /// Axis-aligned box: inside, or within reach of an edge.
    fn bbox(&self, pts: &[Point]) -> bool {
        if pts.is_empty() {
            return true;
        }
        let (mut lo, mut hi) = (pts[0], pts[0]);
        for p in pts {
            lo = Point {
                x: lo.x.min(p.x),
                y: lo.y.min(p.y),
            };
            hi = Point {
                x: hi.x.max(p.x),
                y: hi.y.max(p.y),
            };
        }
        let inside = |p: Point| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
        if self.0.iter().any(|h| inside(h.a) || inside(h.b)) {
            return true;
        }
        let c = [
            lo,
            Point { x: hi.x, y: lo.y },
            hi,
            Point { x: lo.x, y: hi.y },
            lo,
        ];
        self.polyline(&c, 0.)
    }
}

fn json<T: Serialize>(tag: &str, v: &T) -> String {
    format!("{tag}:{}", serde_json::to_string(v).unwrap_or_default())
}

/// Assign engine-3 evidence to every finding.
pub fn assign(board: &Board, extra: &Extra, config: &Config, findings: &mut [Finding]) {
    let zones_json: Vec<_> = extra.zones.iter().map(|z| json("zone", z)).collect();
    for f in findings {
        f.evidence = evidence(board, extra, config, f, &zones_json);
    }
}

fn evidence(b: &Board, x: &Extra, config: &Config, f: &Finding, zones_json: &[String]) -> String {
    let scope = scope(&f.rule);
    let subjects: BTreeSet<&str> = f.subjects.iter().map(String::as_str).collect();
    let nets: BTreeSet<&str> = f
        .nets
        .iter()
        .map(String::as_str)
        .filter(|n| !n.is_empty())
        .collect();
    // Footprints that are subjects directly or through one of their pads.
    let mut refs: BTreeSet<&str> = b
        .parts
        .iter()
        .filter(|p| subjects.contains(p.id.as_str()))
        .map(|p| p.reference.as_str())
        .collect();
    refs.extend(
        b.pads
            .iter()
            .filter(|p| subjects.contains(p.id.as_str()))
            .map(|p| p.reference.as_str()),
    );
    let mut hood = vec![];
    let geometric_subject = f.subjects.iter().any(|s| !s.starts_with("contract:"));
    if geometric_subject || f.at != (Point { x: 0., y: 0. }) {
        hood.push(Anchor {
            a: f.at,
            b: f.at,
            r: 0.,
        });
    }
    for t in b.tracks.iter().filter(|t| subjects.contains(t.id.as_str())) {
        hood.push(Anchor {
            a: t.a,
            b: t.b,
            r: t.width / 2.,
        });
    }
    for v in b.vias.iter().filter(|v| subjects.contains(v.id.as_str())) {
        hood.push(Anchor {
            a: v.at,
            b: v.at,
            r: v.size / 2.,
        });
    }
    for p in b
        .pads
        .iter()
        .filter(|p| subjects.contains(p.id.as_str()) || refs.contains(p.reference.as_str()))
    {
        hood.push(Anchor {
            a: p.at,
            b: p.at,
            r: p.size.x.hypot(p.size.y) / 2.,
        });
    }
    for p in b.parts.iter().filter(|p| subjects.contains(p.id.as_str())) {
        hood.push(Anchor {
            a: p.at,
            b: p.at,
            r: 0.,
        });
    }
    for a in x.arcs.iter().filter(|a| subjects.contains(a.id.as_str())) {
        for w in a.points.windows(2) {
            hood.push(Anchor {
                a: w[0],
                b: w[1],
                r: a.width / 2.,
            });
        }
    }
    for t in x.texts.iter().filter(|t| subjects.contains(t.id.as_str())) {
        hood.push(Anchor {
            a: t.bounds.min,
            b: t.bounds.max,
            r: 0.,
        });
    }
    let hood = Hood(hood);
    let local = scope != Scope::Metadata && !hood.0.is_empty();
    let net_wide = |n: &String| scope == Scope::WholeNet && nets.contains(n.as_str());
    let mut items = vec![];
    for t in &b.tracks {
        if subjects.contains(t.id.as_str())
            || net_wide(&t.net)
            || (local && hood.seg(t.a, t.b, t.width / 2.))
        {
            items.push(json("track", t));
        }
    }
    for v in &b.vias {
        if subjects.contains(v.id.as_str())
            || net_wide(&v.net)
            || (local && hood.point(v.at, v.size / 2.))
        {
            items.push(json("via", v));
        }
    }
    for p in &b.pads {
        let included = subjects.contains(p.id.as_str())
            || refs.contains(p.reference.as_str())
            || net_wide(&p.net)
            || (local && hood.point(p.at, p.size.x.hypot(p.size.y) / 2.));
        if included {
            items.push(json("pad", p));
            if let Some(t) = x.pad_thermals.get(&p.id) {
                items.push(format!("thermal:{}:{t}", p.id));
            }
        }
    }
    for p in &b.parts {
        if subjects.contains(p.id.as_str())
            || refs.contains(p.reference.as_str())
            || (local && hood.point(p.at, 0.))
        {
            items.push(json("part", p));
            if let Some(fields) = x.fields.get(&p.reference) {
                items.push(json(&format!("fields:{}", p.reference), fields));
            }
        }
    }
    let owners: BTreeSet<&str> = b
        .parts
        .iter()
        .filter(|p| refs.contains(p.reference.as_str()) || (local && hood.point(p.at, 0.)))
        .map(|p| p.id.as_str())
        .collect();
    for u in &b.unmodeled_located {
        if owners.contains(u.owner.as_str()) || (local && u.at.is_none_or(|p| hood.point(p, 0.))) {
            items.push(format!("unmodeled:{}", u.hash));
        }
    }
    for (z, text) in x.zones.iter().zip(zones_json) {
        let shares_net = scope != Scope::Metadata && nets.contains(z.net.as_str());
        let near = local && {
            let pts: Vec<Point> = z
                .outlines
                .iter()
                .chain(&z.filled)
                .flatten()
                .copied()
                .collect();
            hood.bbox(&pts)
        };
        if subjects.contains(z.id.as_str()) || shares_net || near {
            items.push(text.clone());
        }
    }
    if local {
        for a in &x.arcs {
            if subjects.contains(a.id.as_str())
                || net_wide(&a.net)
                || hood.polyline(&a.points, a.width / 2.)
            {
                items.push(json("arc", a));
            }
        }
        for t in &x.texts {
            if subjects.contains(t.id.as_str()) || hood.bbox(&[t.bounds.min, t.bounds.max]) {
                items.push(json("text", t));
            }
        }
        for (tag, regions) in [("keepout", &x.keepouts), ("via_keepout", &x.via_keepouts)] {
            for r in regions {
                if hood.bbox(&r.polygon) {
                    items.push(json(tag, r));
                }
            }
        }
        for o in &x.outlines {
            let mut closed = o.clone();
            if let Some(first) = o.first() {
                closed.push(*first);
            }
            if hood.polyline(&closed, 0.) {
                items.push(json("outline", o));
            }
        }
    } else {
        for a in x.arcs.iter().filter(|a| subjects.contains(a.id.as_str())) {
            items.push(json("arc", a));
        }
        for t in x.texts.iter().filter(|t| subjects.contains(t.id.as_str())) {
            items.push(json("text", t));
        }
    }
    items.sort();
    items.dedup();
    let metrics: BTreeMap<&String, &f64> = f.metrics.iter().collect();
    hash(
        &serde_json::to_string(&(
            VERSION,
            &f.rule,
            scope,
            &b.copper_layers,
            &items,
            config_slice(&f.rule, config),
            metrics,
        ))
        .unwrap_or_default(),
    )
}
