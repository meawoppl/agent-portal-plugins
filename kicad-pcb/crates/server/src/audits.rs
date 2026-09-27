//! In-plugin layout-quality audits over the `.kicad_pcb` s-expression.
//!
//! These cover playbook profile items that kicad-tools does not (yet) check.
//! Each audit declares the kct rule ids that would supersede it; when kct
//! reports one of those rules the audit is skipped automatically, so audits
//! retire themselves as upstream rules land. All geometry is approximate and
//! deliberately conservative (prefer missing a finding over a false one).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    profile::{Level, Profile},
    quality::{Finding, FindingItem},
    sexpr::Node,
};

/// Audit ids, the profile item each maps to, and the kct rule ids that
/// supersede it.
pub(crate) const AUDITS: &[AuditSpec] = &[
    AuditSpec {
        id: "via_under_package",
        profile_item: "vias.under_package_body",
        kct_equivalents: &["via_under_package", "via_under_body", "via_under_component"],
    },
    AuditSpec {
        id: "off_angle_track",
        profile_item: "routing.octilinear_only",
        kct_equivalents: &["off_angle_track", "track_off_axis", "octilinear"],
    },
    AuditSpec {
        id: "orphan_via",
        profile_item: "vias.orphans",
        kct_equivalents: &["orphan_via", "via_orphan"],
    },
    AuditSpec {
        id: "silk_reference_prefix",
        profile_item: "silkscreen.reference_prefixes",
        kct_equivalents: &["silk_reference_prefix", "silkscreen_reference_prefix"],
    },
    AuditSpec {
        id: "silk_text_size",
        profile_item: "silkscreen.text_height_mm",
        kct_equivalents: &["silk_text_size", "silkscreen_text_uniform"],
    },
    AuditSpec {
        id: "silk_explanatory_text",
        profile_item: "silkscreen.explanatory_text",
        kct_equivalents: &["silk_explanatory_text", "silkscreen_explanatory_text"],
    },
    AuditSpec {
        id: "decoupling_distance",
        profile_item: "decoupling.max_pin_distance_mm",
        kct_equivalents: &["decoupling_distance", "decoupling_pin_distance"],
    },
];

#[derive(Debug)]
pub(crate) struct AuditSpec {
    pub id: &'static str,
    pub profile_item: &'static str,
    pub kct_equivalents: &'static [&'static str],
}

type Pt = (f64, f64);

#[derive(Debug, Default)]
pub(crate) struct Board {
    /// Copper layers in stack order.
    pub copper: Vec<String>,
    pub footprints: Vec<Footprint>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    pub zones: Vec<Zone>,
    /// Board-level text (gr_text) on any layer.
    pub texts: Vec<Text>,
}

#[derive(Debug, Default)]
pub(crate) struct Footprint {
    pub reference: String,
    pub value: String,
    pub lib_id: String,
    pub at: Pt,
    pub rot: f64,
    pub back: bool,
    pub pads: Vec<Pad>,
    /// Package body extents in footprint-local coordinates (from Fab).
    pub fab: Option<Rect>,
    pub texts: Vec<Text>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Pad {
    pub number: String,
    pub net: Option<String>,
    pub local: Pt,
    pub size: Pt,
    /// Pad rotation relative to the footprint.
    pub rot: f64,
    pub layers: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Track {
    pub start: Pt,
    pub end: Pt,
    pub width: f64,
    pub layer: String,
    pub net: Option<String>,
    pub arc: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Via {
    pub at: Pt,
    pub size: f64,
    pub layers: (String, String),
    pub net: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Zone {
    pub net: Option<String>,
    pub layers: Vec<String>,
    pub outline: Vec<Pt>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Text {
    /// `reference`, `value`, `user` (footprint), or `board`.
    pub kind: String,
    pub text: String,
    pub layer: String,
    pub height: f64,
    pub width: f64,
    pub thickness: Option<f64>,
    pub hidden: bool,
    pub pos: Pt,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub min: Pt,
    pub max: Pt,
}

impl Rect {
    fn from_points(points: impl IntoIterator<Item = Pt>) -> Option<Rect> {
        let mut rect: Option<Rect> = None;
        for (x, y) in points {
            rect = Some(match rect {
                None => Rect {
                    min: (x, y),
                    max: (x, y),
                },
                Some(r) => Rect {
                    min: (r.min.0.min(x), r.min.1.min(y)),
                    max: (r.max.0.max(x), r.max.1.max(y)),
                },
            });
        }
        rect.filter(|r| r.max.0 - r.min.0 > 1e-6 && r.max.1 - r.min.1 > 1e-6)
    }

    fn contains_strict(&self, p: Pt) -> bool {
        p.0 > self.min.0 && p.0 < self.max.0 && p.1 > self.min.1 && p.1 < self.max.1
    }
}

// ---------------------------------------------------------------------------
// Parsing

fn net_name(node: &Node, table: &HashMap<String, String>) -> Option<String> {
    let net = node.child("net")?;
    let name = match (net.arg(1), net.arg(2)) {
        (_, Some(name)) => name.to_string(),
        (Some(first), None) => table
            .get(first)
            .cloned()
            .unwrap_or_else(|| first.to_string()),
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

fn text_from(node: &Node, kind: &str, text: &str) -> Text {
    let (pos, _) = at(node);
    let font = node
        .child("effects")
        .and_then(|effects| effects.child("font"));
    let size = font.and_then(|font| font.child("size"));
    let effects_hidden = node.child("effects").map(Node::hidden).unwrap_or(false);
    Text {
        kind: kind.to_string(),
        text: text.to_string(),
        layer: node.value("layer").unwrap_or_default().to_string(),
        height: size.and_then(|s| s.num(1)).unwrap_or(0.0),
        width: size.and_then(|s| s.num(2)).unwrap_or(0.0),
        thickness: font
            .and_then(|font| font.child("thickness"))
            .and_then(|t| t.num(1)),
        hidden: node.hidden() || effects_hidden,
        pos,
    }
}

fn at(node: &Node) -> (Pt, f64) {
    match node.child("at") {
        Some(at) => (
            (at.num(1).unwrap_or(0.0), at.num(2).unwrap_or(0.0)),
            at.num(3).unwrap_or(0.0),
        ),
        None => ((0.0, 0.0), 0.0),
    }
}

fn shape_points(node: &Node) -> Vec<Pt> {
    let mut points = Vec::new();
    for key in ["start", "end", "mid", "center"] {
        if let Some(point) = node.point(key) {
            points.push(point);
        }
    }
    if let Some(pts) = node.child("pts") {
        points.extend(
            pts.children("xy")
                .filter_map(|xy| Some((xy.num(1)?, xy.num(2)?))),
        );
    }
    if node.is("fp_circle") {
        if let (Some(c), Some(e)) = (node.point("center"), node.point("end")) {
            let r = ((e.0 - c.0).powi(2) + (e.1 - c.1).powi(2)).sqrt();
            points.extend([(c.0 - r, c.1 - r), (c.0 + r, c.1 + r)]);
        }
    }
    points
}

fn parse_footprint(node: &Node, nets: &HashMap<String, String>) -> Footprint {
    let (pos, rot) = at(node);
    let layer = node.value("layer").unwrap_or("F.Cu");
    let mut fp = Footprint {
        lib_id: node.arg(1).unwrap_or_default().to_string(),
        at: pos,
        rot,
        back: layer.starts_with("B."),
        ..Footprint::default()
    };
    let mut fab_points = Vec::new();
    for child in node.items() {
        match child.head() {
            Some("property") => {
                let name = child.arg(1).unwrap_or_default();
                let value = child.arg(2).unwrap_or_default();
                if name == "Reference" {
                    fp.reference = value.to_string();
                } else if name == "Value" {
                    fp.value = value.to_string();
                }
                let kind = match name {
                    "Reference" => "reference",
                    "Value" => "value",
                    _ => "field",
                };
                fp.texts.push(text_from(child, kind, value));
            }
            Some("fp_text") => {
                let kind = child.arg(1).unwrap_or("user");
                let value = child.arg(2).unwrap_or_default();
                if kind == "reference" && fp.reference.is_empty() {
                    fp.reference = value.to_string();
                } else if kind == "value" && fp.value.is_empty() {
                    fp.value = value.to_string();
                }
                fp.texts.push(text_from(child, kind, value));
            }
            Some("fp_line" | "fp_rect" | "fp_poly" | "fp_circle" | "fp_arc") => {
                if child.value("layer").is_some_and(|l| l.ends_with(".Fab")) {
                    fab_points.extend(shape_points(child));
                }
            }
            Some("pad") => {
                let (local, pad_rot) = at(child);
                let size = child
                    .child("size")
                    .map(|s| (s.num(1).unwrap_or(0.0), s.num(2).unwrap_or(0.0)))
                    .unwrap_or_default();
                fp.pads.push(Pad {
                    number: child.arg(1).unwrap_or_default().to_string(),
                    net: net_name(child, nets),
                    local,
                    size,
                    rot: pad_rot - rot,
                    layers: child
                        .child("layers")
                        .map(|l| l.args().map(str::to_string).collect())
                        .unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    fp.fab = Rect::from_points(fab_points);
    fp
}

pub(crate) fn parse_board(root: &Node) -> Board {
    let mut board = Board::default();
    let mut nets = HashMap::new();
    for net in root.children("net") {
        if let (Some(id), Some(name)) = (net.arg(1), net.arg(2)) {
            nets.insert(id.to_string(), name.to_string());
        }
    }
    if let Some(layers) = root.child("layers") {
        for layer in layers.items().iter().skip(1) {
            if let Some(name) = layer.arg(1).filter(|name| name.ends_with(".Cu")) {
                board.copper.push(name.to_string());
            }
        }
    }
    for child in root.items() {
        match child.head() {
            Some("footprint" | "module") => board.footprints.push(parse_footprint(child, &nets)),
            Some(kind @ ("segment" | "arc")) => {
                let (Some(start), Some(end)) = (child.point("start"), child.point("end")) else {
                    continue;
                };
                board.tracks.push(Track {
                    start,
                    end,
                    width: child.child("width").and_then(|w| w.num(1)).unwrap_or(0.0),
                    layer: child.value("layer").unwrap_or_default().to_string(),
                    net: net_name(child, &nets),
                    arc: kind == "arc",
                });
            }
            Some("via") => {
                let (pos, _) = at(child);
                let layers: Vec<String> = child
                    .child("layers")
                    .map(|l| l.args().map(str::to_string).collect())
                    .unwrap_or_default();
                let first = layers.first().cloned().unwrap_or_else(|| "F.Cu".into());
                let last = layers.last().cloned().unwrap_or_else(|| "B.Cu".into());
                board.vias.push(Via {
                    at: pos,
                    size: child.child("size").and_then(|s| s.num(1)).unwrap_or(0.0),
                    layers: (first, last),
                    net: net_name(child, &nets),
                });
            }
            Some("zone") => {
                let keepout = child.child("keepout").is_some();
                let net = net_name(child, &nets).or_else(|| {
                    child
                        .value("net_name")
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                });
                if keepout || net.is_none() {
                    continue;
                }
                let mut layers: Vec<String> = child
                    .child("layers")
                    .map(|l| l.args().map(str::to_string).collect())
                    .unwrap_or_default();
                if let Some(layer) = child.value("layer") {
                    layers.push(layer.to_string());
                }
                let outline = child
                    .child("polygon")
                    .and_then(|p| p.child("pts"))
                    .map(|pts| {
                        pts.children("xy")
                            .filter_map(|xy| Some((xy.num(1)?, xy.num(2)?)))
                            .collect()
                    })
                    .unwrap_or_default();
                board.zones.push(Zone {
                    net,
                    layers,
                    outline,
                });
            }
            Some("gr_text" | "gr_text_box") => {
                let text = child.arg(1).unwrap_or_default();
                board.texts.push(text_from(child, "board", text));
            }
            _ => {}
        }
    }
    board
}

// ---------------------------------------------------------------------------
// Geometry helpers

/// Footprint-local point to board coordinates (KiCad rotation convention,
/// y axis down).
fn to_board(fp: &Footprint, p: Pt) -> Pt {
    let (s, c) = fp.rot.to_radians().sin_cos();
    (fp.at.0 + p.0 * c + p.1 * s, fp.at.1 - p.0 * s + p.1 * c)
}

fn to_local(fp: &Footprint, p: Pt) -> Pt {
    let (s, c) = fp.rot.to_radians().sin_cos();
    let (dx, dy) = (p.0 - fp.at.0, p.1 - fp.at.1);
    (dx * c - dy * s, dx * s + dy * c)
}

/// Pad rectangle half-extents in footprint-local axes (rotations snapped to
/// 90 degrees; other angles use the bounding square).
fn pad_half_extents(pad: &Pad) -> Pt {
    let r = pad.rot.rem_euclid(180.0);
    if (r - 90.0).abs() < 1.0 {
        (pad.size.1 / 2.0, pad.size.0 / 2.0)
    } else if r < 1.0 || r > 179.0 {
        (pad.size.0 / 2.0, pad.size.1 / 2.0)
    } else {
        let m = pad.size.0.max(pad.size.1) / 2.0;
        (m, m)
    }
}

fn pad_contains(fp: &Footprint, pad: &Pad, p: Pt, margin: f64) -> bool {
    let local = to_local(fp, p);
    let (hx, hy) = pad_half_extents(pad);
    (local.0 - pad.local.0).abs() <= hx + margin && (local.1 - pad.local.1).abs() <= hy + margin
}

fn layer_matches(pattern: &str, layer: &str) -> bool {
    pattern == layer
        || pattern == "*.Cu"
        || (pattern == "F&B.Cu" && (layer == "F.Cu" || layer == "B.Cu"))
}

fn dist(a: Pt, b: Pt) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-12 {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

fn point_in_polygon(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn ref_prefix(reference: &str) -> String {
    reference
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn item(description: impl Into<String>, pos: Option<Pt>) -> FindingItem {
    FindingItem {
        description: description.into(),
        pos: pos.map(|(x, y)| crate::quality::Pos {
            x: round(x),
            y: round(y),
        }),
    }
}

fn finding(rule: &str, default: Level, description: String, items: Vec<FindingItem>) -> Finding {
    Finding::new(rule, default, description, "plugin", items)
}

// ---------------------------------------------------------------------------
// Audits

pub(crate) fn run(board: &Board, profile: &Profile, skip: &HashSet<&str>) -> Vec<Finding> {
    let mut out = Vec::new();
    for spec in AUDITS {
        if skip.contains(spec.id) {
            continue;
        }
        let mut findings = match spec.id {
            "via_under_package" => via_under_package(board, profile),
            "off_angle_track" => off_angle_tracks(board, profile),
            "orphan_via" => orphan_vias(board),
            "silk_reference_prefix" => silk_reference_prefix(board, profile),
            "silk_text_size" => silk_text_size(board, profile),
            "silk_explanatory_text" => silk_explanatory_text(board),
            "decoupling_distance" => decoupling_distance(board, profile),
            _ => Vec::new(),
        };
        for finding in &mut findings {
            finding.profile_item = Some(spec.profile_item.to_string());
        }
        out.extend(findings);
    }
    out
}

pub(crate) fn via_under_package(board: &Board, profile: &Profile) -> Vec<Finding> {
    let families = profile.body_keepout_packages();
    let mut out = Vec::new();
    for fp in &board.footprints {
        let name = fp
            .lib_id
            .rsplit(':')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let Some(family) = families
            .iter()
            .find(|family| name.contains(family.as_str()))
        else {
            continue;
        };
        let body = fp.fab.or_else(|| {
            Rect::from_points(fp.pads.iter().flat_map(|pad| {
                let (hx, hy) = pad_half_extents(pad);
                [
                    (pad.local.0 - hx, pad.local.1 - hy),
                    (pad.local.0 + hx, pad.local.1 + hy),
                ]
            }))
        });
        let Some(body) = body else { continue };
        for via in &board.vias {
            let local = to_local(fp, via.at);
            if !body.contains_strict(local) {
                continue;
            }
            let pad = fp
                .pads
                .iter()
                .find(|pad| pad_contains(fp, pad, via.at, 0.0))
                .map(|pad| format!(" (in pad {})", pad.number))
                .unwrap_or_default();
            out.push(finding(
                "via_under_package",
                Level::Error,
                format!("Via under {} {family} package body{pad}", fp.reference),
                vec![item(
                    format!(
                        "via {} on {}",
                        via.net.as_deref().unwrap_or("<no net>"),
                        fp.reference
                    ),
                    Some(via.at),
                )],
            ));
        }
    }
    out
}

pub(crate) fn off_angle_tracks(board: &Board, profile: &Profile) -> Vec<Finding> {
    if profile.bool("routing.octilinear_only") == Some(false) {
        return Vec::new();
    }
    const TOLERANCE_DEG: f64 = 0.5;
    const MIN_LENGTH_MM: f64 = 0.05;
    let mut out = Vec::new();
    for track in board.tracks.iter().filter(|track| !track.arc) {
        let (dx, dy) = (track.end.0 - track.start.0, track.end.1 - track.start.1);
        if (dx * dx + dy * dy).sqrt() < MIN_LENGTH_MM {
            continue;
        }
        let angle = dy.atan2(dx).to_degrees().rem_euclid(45.0);
        let off = angle.min(45.0 - angle);
        if off <= TOLERANCE_DEG {
            continue;
        }
        let absolute = dy.atan2(dx).to_degrees().rem_euclid(180.0);
        out.push(finding(
            "off_angle_track",
            Level::Warning,
            format!(
                "Track segment at {:.1} deg on {} ({})",
                absolute,
                track.layer,
                track.net.as_deref().unwrap_or("<no net>")
            ),
            vec![
                item("start", Some(track.start)),
                item("end", Some(track.end)),
            ],
        ));
    }
    out
}

fn via_span<'a>(board: &'a Board, via: &Via) -> Vec<&'a str> {
    let index = |name: &str| board.copper.iter().position(|layer| layer == name);
    match (index(&via.layers.0), index(&via.layers.1)) {
        (Some(a), Some(b)) => board.copper[a.min(b)..=a.max(b)]
            .iter()
            .map(String::as_str)
            .collect(),
        _ => board.copper.iter().map(String::as_str).collect(),
    }
}

/// A via is an orphan when same-net copper touches it on fewer than two
/// layers. Zones count by outline (not fill), which errs toward "connected".
pub(crate) fn orphan_vias(board: &Board) -> Vec<Finding> {
    let mut out = Vec::new();
    for via in &board.vias {
        let radius = via.size / 2.0;
        let Some(net) = via.net.as_deref() else {
            out.push(finding(
                "orphan_via",
                Level::Error,
                "Via has no net".to_string(),
                vec![item("via", Some(via.at))],
            ));
            continue;
        };
        let mut touched = Vec::new();
        for layer in via_span(board, via) {
            let track = board.tracks.iter().any(|track| {
                track.layer == layer
                    && track.net.as_deref() == Some(net)
                    && dist_to_segment(via.at, track.start, track.end)
                        <= radius + track.width / 2.0 + 1e-3
            });
            let pad = track
                || board.footprints.iter().any(|fp| {
                    fp.pads.iter().any(|pad| {
                        pad.net.as_deref() == Some(net)
                            && pad.layers.iter().any(|l| layer_matches(l, layer))
                            && pad_contains(fp, pad, via.at, radius)
                    })
                });
            let zone = pad
                || board.zones.iter().any(|zone| {
                    zone.net.as_deref() == Some(net)
                        && zone.layers.iter().any(|l| layer_matches(l, layer))
                        && point_in_polygon(via.at, &zone.outline)
                });
            if zone {
                touched.push(layer);
            }
        }
        if touched.len() < 2 {
            out.push(finding(
                "orphan_via",
                Level::Error,
                format!(
                    "Via on {net} connects {} layer{} ({})",
                    touched.len(),
                    if touched.len() == 1 { "" } else { "s" },
                    if touched.is_empty() {
                        "nothing".to_string()
                    } else {
                        touched.join(", ")
                    }
                ),
                vec![item(format!("via {net}"), Some(via.at))],
            ));
        }
    }
    out
}

fn is_silk(layer: &str) -> bool {
    layer.ends_with(".SilkS") || layer.ends_with(".Silkscreen")
}

pub(crate) fn silk_reference_prefix(board: &Board, profile: &Profile) -> Vec<Finding> {
    let Some(allowed) = profile.silk_reference_prefixes() else {
        return Vec::new();
    };
    let allowed: HashSet<String> = allowed.iter().map(|p| p.to_ascii_uppercase()).collect();
    let mut out = Vec::new();
    for fp in &board.footprints {
        let prefix = ref_prefix(&fp.reference);
        if prefix.is_empty() || allowed.contains(&prefix) {
            continue;
        }
        for text in fp.texts.iter().filter(|t| t.kind == "reference") {
            if text.hidden || !is_silk(&text.layer) {
                continue;
            }
            out.push(finding(
                "silk_reference_prefix",
                Level::Warning,
                format!(
                    "{} reference is visible on {}; profile shows only {} prefixes",
                    fp.reference,
                    text.layer,
                    {
                        let mut list: Vec<_> = allowed.iter().cloned().collect();
                        list.sort();
                        list.join("/")
                    }
                ),
                vec![item(fp.reference.clone(), Some(to_board(fp, text.pos)))],
            ));
        }
    }
    out
}

/// Visible silkscreen text: (owner, text, board position).
fn visible_silk(board: &Board) -> Vec<(String, &Text, Pt)> {
    let mut texts = Vec::new();
    for fp in &board.footprints {
        for text in &fp.texts {
            if !text.hidden && is_silk(&text.layer) && !text.text.is_empty() {
                texts.push((fp.reference.clone(), text, to_board(fp, text.pos)));
            }
        }
    }
    for text in &board.texts {
        if !text.hidden && is_silk(&text.layer) {
            texts.push(("board".to_string(), text, text.pos));
        }
    }
    texts
}

pub(crate) fn silk_text_size(board: &Board, profile: &Profile) -> Vec<Finding> {
    let min_height = profile.f64("silkscreen.text_height_mm");
    let min_thickness = profile.f64("silkscreen.text_thickness_mm");
    let uniform = profile.bool("silkscreen.uniform_size").unwrap_or(false);
    let texts = visible_silk(board);
    let mut out = Vec::new();
    let key = |t: &Text| {
        (
            (t.height * 1000.0).round() as i64,
            (t.width * 1000.0).round() as i64,
            (t.thickness.unwrap_or(0.0) * 1000.0).round() as i64,
        )
    };
    let mut counts: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
    for (_, text, _) in &texts {
        *counts.entry(key(text)).or_default() += 1;
    }
    let dominant = counts
        .iter()
        .max_by_key(|(size, count)| (**count, std::cmp::Reverse(**size)))
        .map(|(size, _)| *size);
    for (owner, text, pos) in &texts {
        let label = format!("{owner} {} \"{}\"", text.kind, text.text);
        let describe = format!(
            "{:.2}x{:.2} mm, {:.3} mm stroke",
            text.height,
            text.width,
            text.thickness.unwrap_or(0.0)
        );
        let small_height = min_height.is_some_and(|min| text.height + 1e-3 < min);
        let thin = min_thickness
            .zip(text.thickness)
            .is_some_and(|(min, thickness)| thickness + 1e-4 < min);
        if small_height || thin {
            out.push(finding(
                "silk_text_below_min",
                Level::Warning,
                format!(
                    "Silkscreen text below profile minimum ({describe}; min {} mm height, {} mm stroke)",
                    min_height.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                    min_thickness.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
                ),
                vec![item(label, Some(*pos))],
            ));
        } else if uniform && dominant.is_some_and(|size| size != key(text)) {
            let (h, w, t) = dominant.unwrap_or_default();
            out.push(finding(
                "silk_text_nonuniform",
                Level::Info,
                format!(
                    "Silkscreen text size differs from board majority ({describe} vs {:.2}x{:.2} mm, {:.3} mm)",
                    h as f64 / 1000.0,
                    w as f64 / 1000.0,
                    t as f64 / 1000.0
                ),
                vec![item(label, Some(*pos))],
            ));
        }
    }
    out
}

pub(crate) fn silk_explanatory_text(board: &Board) -> Vec<Finding> {
    let mut out = Vec::new();
    for fp in &board.footprints {
        for text in &fp.texts {
            if text.kind == "user"
                && !text.hidden
                && is_silk(&text.layer)
                && !text.text.contains("${")
                && !text.text.trim().is_empty()
            {
                out.push(finding(
                    "silk_explanatory_text",
                    Level::Warning,
                    format!(
                        "Footprint silkscreen text \"{}\" on {}",
                        text.text, fp.reference
                    ),
                    vec![item(fp.reference.clone(), Some(to_board(fp, text.pos)))],
                ));
            }
        }
    }
    for text in &board.texts {
        if !text.hidden && is_silk(&text.layer) && !text.text.contains("${") {
            out.push(finding(
                "silk_explanatory_text",
                Level::Warning,
                format!("Board silkscreen text \"{}\" on {}", text.text, text.layer),
                vec![item(text.text.clone(), Some(text.pos))],
            ));
        }
    }
    out
}

/// Capacitance in farads from values like `100nF`, `4u7`, `10uF/16V`, `12pF`.
pub(crate) fn capacitance(value: &str) -> Option<f64> {
    let value = value.trim().replace('µ', "u");
    let digits: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let rest = &value[digits.len()..];
    let unit = rest.chars().next()?;
    let scale = match unit.to_ascii_lowercase() {
        'p' => 1e-12,
        'n' => 1e-9,
        'u' => 1e-6,
        'm' => 1e-3,
        _ => return None,
    };
    let tail: String = rest[1..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let number: f64 = if tail.is_empty() {
        digits.parse().ok()?
    } else {
        format!("{digits}.{tail}").parse().ok()?
    };
    Some(number * scale)
}

fn is_ground(net: &str) -> bool {
    let upper = net.to_ascii_uppercase();
    upper.contains("GND") || upper.contains("VSS")
}

/// Every IC pin on a decoupled supply net (a non-ground net that some
/// capacitor of at least 10 nF bridges to ground; smaller values are load or
/// filter caps) needs a capacitor pad on that net within
/// `decoupling.max_pin_distance_mm`, on the same side when
/// `same_side_as_ic` is set.
pub(crate) fn decoupling_distance(board: &Board, profile: &Profile) -> Vec<Finding> {
    let Some(limit) = profile.f64("decoupling.max_pin_distance_mm") else {
        return Vec::new();
    };
    let same_side = profile.bool("decoupling.same_side_as_ic").unwrap_or(false);
    // (net, board position, back side, cap ref)
    let mut cap_pads: Vec<(&str, Pt, bool, &str)> = Vec::new();
    let mut supply: HashSet<&str> = HashSet::new();
    let decoupler = |fp: &&Footprint| {
        ref_prefix(&fp.reference) == "C"
            && capacitance(&fp.value).is_none_or(|farads| farads >= 10e-9)
    };
    for fp in board.footprints.iter().filter(decoupler) {
        let nets: Vec<&str> = fp.pads.iter().filter_map(|p| p.net.as_deref()).collect();
        if !nets.iter().any(|net| is_ground(net)) {
            continue;
        }
        for pad in &fp.pads {
            if let Some(net) = pad.net.as_deref().filter(|net| !is_ground(net)) {
                supply.insert(net);
                cap_pads.push((net, to_board(fp, pad.local), fp.back, &fp.reference));
            }
        }
    }
    let mut out = Vec::new();
    for fp in board
        .footprints
        .iter()
        .filter(|fp| matches!(ref_prefix(&fp.reference).as_str(), "U" | "IC"))
    {
        for pad in &fp.pads {
            let Some(net) = pad.net.as_deref().filter(|net| supply.contains(net)) else {
                continue;
            };
            let pos = to_board(fp, pad.local);
            let nearest = |same: Option<bool>| {
                cap_pads
                    .iter()
                    .filter(|(n, _, back, _)| {
                        *n == net && same.is_none_or(|s| (*back == fp.back) == s)
                    })
                    .map(|(_, p, _, cap)| (dist(pos, *p), *cap))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            };
            let best = if same_side {
                nearest(Some(true))
            } else {
                nearest(None)
            };
            if best.is_some_and(|(d, _)| d <= limit) {
                continue;
            }
            let detail = match best {
                Some((d, cap)) => format!("nearest {cap} at {d:.2} mm"),
                None => "no capacitor on this side".to_string(),
            };
            let other = if same_side {
                nearest(Some(false))
                    .filter(|(d, _)| *d <= limit)
                    .map(|(d, cap)| format!("; {cap} is {d:.2} mm away on the opposite side"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            out.push(finding(
                "decoupling_distance",
                Level::Warning,
                format!(
                    "{} pin {} ({net}) has no decoupler within {limit} mm ({detail}{other})",
                    fp.reference, pad.number
                ),
                vec![item(format!("{}.{}", fp.reference, pad.number), Some(pos))],
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr;

    fn board(body: &str) -> Board {
        let text = format!(
            r#"(kicad_pcb (version 20260206)
  (layers (0 "F.Cu" signal) (4 "In1.Cu" signal) (2 "B.Cu" signal) (5 "F.SilkS" user "F.Silkscreen"))
  {body})"#
        );
        parse_board(&sexpr::parse(&text).unwrap())
    }

    fn profile(text: &str) -> Profile {
        Profile::parse(text).unwrap()
    }

    const QFN: &str = r#"(footprint "Pkg:QFN-16_3x3mm" (layer "F.Cu") (at 10 10 90)
      (property "Reference" "U1" (at 0 -2.5 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
      (fp_rect (start -1.5 -1.5) (end 1.5 1.5) (layer "F.Fab"))
      (pad "17" smd rect (at 0 0 90) (size 1.6 1.6) (layers "F.Cu") (net "GND"))
      (pad "1" smd rect (at -1.45 -0.75 90) (size 0.25 0.7) (layers "F.Cu") (net "VCC")))"#;

    #[test]
    fn via_under_package_uses_fab_body_and_rotation() {
        let b = board(&format!(
            r#"{QFN}
            (via (at 10.5 10.5) (size 0.45) (drill 0.2) (layers "F.Cu" "B.Cu") (net "GND"))
            (via (at 10 10) (size 0.45) (drill 0.2) (layers "F.Cu" "B.Cu") (net "GND"))
            (via (at 12 10) (size 0.45) (drill 0.2) (layers "F.Cu" "B.Cu") (net "GND"))"#
        ));
        let found = via_under_package(&b, &Profile::default());
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[1].description.contains("in pad 17"));
        assert!(found.iter().all(|f| f.description.contains("U1 QFN")));
    }

    #[test]
    fn via_under_package_ignores_other_families() {
        let b = board(
            r#"(footprint "Pkg:SOIC-8" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U2" (at 0 0) (layer "F.Fab"))
                 (fp_rect (start -2 -2) (end 2 2) (layer "F.Fab")))
               (via (at 0 0) (size 0.45) (layers "F.Cu" "B.Cu") (net "GND"))"#,
        );
        assert!(via_under_package(&b, &Profile::default()).is_empty());
        let p = profile("routing:\n  no_front_routing_under: [soic]\n");
        assert_eq!(via_under_package(&b, &p).len(), 1);
    }

    #[test]
    fn off_angle_tracks_flags_non_octilinear_segments() {
        let b = board(
            r#"(segment (start 0 0) (end 5 0) (width 0.2) (layer "F.Cu") (net "A"))
               (segment (start 0 0) (end 3 3) (width 0.2) (layer "F.Cu") (net "A"))
               (segment (start 0 0) (end 0 -4) (width 0.2) (layer "F.Cu") (net "A"))
               (segment (start 0 0) (end 4 1) (width 0.2) (layer "B.Cu") (net "B"))
               (segment (start 0 0) (end 0.01 0.003) (width 0.2) (layer "B.Cu") (net "B"))
               (arc (start 0 0) (mid 1 0.4) (end 2 0) (width 0.2) (layer "F.Cu") (net "A"))"#,
        );
        let found = off_angle_tracks(&b, &Profile::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].description.contains("14.0 deg"));
        assert!(off_angle_tracks(&b, &profile("routing:\n  octilinear_only: false\n")).is_empty());
    }

    #[test]
    fn orphan_vias_need_two_connected_layers() {
        let b = board(
            r#"(footprint "R" (layer "F.Cu") (at 0 0)
                 (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net "A")))
               (zone (net "A") (layer "B.Cu") (polygon (pts (xy -5 -5) (xy 5 -5) (xy 5 5) (xy -5 5))))
               (via (at 0.2 0) (size 0.45) (layers "F.Cu" "B.Cu") (net "A"))
               (segment (start 3 0) (end 8 0) (width 0.2) (layer "F.Cu") (net "A"))
               (via (at 3 0) (size 0.45) (layers "F.Cu" "B.Cu") (net "A"))
               (via (at 20 20) (size 0.45) (layers "F.Cu" "B.Cu") (net "A"))
               (segment (start 20 20) (end 25 20) (width 0.2) (layer "In1.Cu") (net "B"))
               (via (at 30 30) (size 0.45) (layers "F.Cu" "B.Cu"))"#,
        );
        let found = orphan_vias(&b);
        let text: Vec<_> = found.iter().map(|f| f.description.as_str()).collect();
        assert_eq!(
            text,
            ["Via on A connects 0 layers (nothing)", "Via has no net"],
            "{found:?}"
        );
    }

    #[test]
    fn orphan_via_single_layer_is_reported() {
        let b = board(
            r#"(segment (start 0 0) (end 5 0) (width 0.2) (layer "F.Cu") (net "A"))
               (via (at 0 0) (size 0.45) (layers "F.Cu" "B.Cu") (net "A"))"#,
        );
        let found = orphan_vias(&b);
        assert_eq!(found.len(), 1);
        assert!(found[0].description.contains("1 layer (F.Cu)"));
    }

    #[test]
    fn legacy_numeric_nets_resolve_through_table() {
        let b = board(
            r#"(net 0 "") (net 1 "GND")
               (segment (start 0 0) (end 5 0) (width 0.2) (layer "F.Cu") (net 1))
               (segment (start 0 0) (end 5 0) (width 0.2) (layer "B.Cu") (net 1))
               (via (at 0 0) (size 0.45) (layers "F.Cu" "B.Cu") (net 1))"#,
        );
        assert_eq!(b.vias[0].net.as_deref(), Some("GND"));
        assert!(orphan_vias(&b).is_empty());
    }

    #[test]
    fn silk_reference_prefix_flags_visible_hidden_class_refs() {
        let b = board(&format!(
            r#"{QFN}
            (footprint "C_0402" (layer "F.Cu") (at 0 0)
              (property "Reference" "C1" (at 0 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)))))
            (footprint "R_0402" (layer "F.Cu") (at 0 0)
              (property "Reference" "R1" (at 0 0) (layer "F.SilkS") (hide yes) (effects (font (size 1 1)))))"#
        ));
        let p = profile("silkscreen:\n  reference_prefixes: [U, J]\n");
        let found = silk_reference_prefix(&b, &p);
        assert_eq!(found.len(), 1);
        assert!(found[0].description.starts_with("C1 reference"));
        assert!(silk_reference_prefix(&b, &Profile::default()).is_empty());
    }

    #[test]
    fn silk_text_size_reports_small_and_nonuniform() {
        let b = board(
            r#"(footprint "A" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U1" (at 0 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)))))
               (footprint "A" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U2" (at 0 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)))))
               (footprint "A" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U3" (at 0 0) (layer "F.SilkS") (effects (font (size 0.8 0.8) (thickness 0.15)))))
               (footprint "A" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U4" (at 0 0) (layer "F.SilkS") (effects (font (size 1.2 1.2) (thickness 0.18)))))"#,
        );
        let p = profile(
            "silkscreen:\n  text_height_mm: 1.0\n  text_thickness_mm: 0.15\n  uniform_size: true\n",
        );
        let found = silk_text_size(&b, &p);
        let rules: Vec<_> = found.iter().map(|f| f.rule.as_str()).collect();
        assert_eq!(rules, ["silk_text_below_min", "silk_text_nonuniform"]);
    }

    #[test]
    fn silk_explanatory_text_finds_board_and_user_text() {
        let b = board(
            r#"(gr_text "RF KEEP CLEAR" (at 1 1) (layer "F.SilkS") (effects (font (size 1 1))))
               (gr_text "notes" (at 1 1) (layer "Cmts.User"))
               (footprint "A" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U1" (at 0 0) (layer "F.SilkS"))
                 (fp_text user "EXT" (at 0 1) (layer "F.SilkS") (effects (font (size 1 1))))
                 (fp_text user "${REFERENCE}" (at 0 1) (layer "F.SilkS")))"#,
        );
        assert_eq!(silk_explanatory_text(&b).len(), 2);
    }

    #[test]
    fn decoupling_distance_measures_supply_pins() {
        let b = board(
            r#"(footprint "IC" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U1" (at 0 0) (layer "F.Fab"))
                 (pad "1" smd rect (at 0 0) (size 0.3 0.3) (layers "F.Cu") (net "VCC"))
                 (pad "2" smd rect (at 5 0) (size 0.3 0.3) (layers "F.Cu") (net "VCC"))
                 (pad "3" smd rect (at 0 5) (size 0.3 0.3) (layers "F.Cu") (net "SIG"))
                 (pad "4" smd rect (at 10 0) (size 0.3 0.3) (layers "F.Cu") (net "VDDIO")))
               (footprint "C" (layer "F.Cu") (at 1 0)
                 (property "Reference" "C1" (at 0 0) (layer "F.Fab"))
                 (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "F.Cu") (net "VCC"))
                 (pad "2" smd rect (at 0 1) (size 0.5 0.5) (layers "F.Cu") (net "GND")))
               (footprint "C" (layer "B.Cu") (at 10 1)
                 (property "Reference" "C2" (at 0 0) (layer "B.Fab"))
                 (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "B.Cu") (net "VDDIO"))
                 (pad "2" smd rect (at 0 1) (size 0.5 0.5) (layers "B.Cu") (net "GND")))"#,
        );
        let p = profile("decoupling:\n  max_pin_distance_mm: 2.0\n  same_side_as_ic: true\n");
        let found = decoupling_distance(&b, &p);
        let text: Vec<_> = found.iter().map(|f| f.description.as_str()).collect();
        assert_eq!(text.len(), 2, "{text:?}");
        assert!(text[0].starts_with("U1 pin 2 (VCC)") && text[0].contains("nearest C1 at 4.00 mm"));
        assert!(text[1].starts_with("U1 pin 4 (VDDIO)") && text[1].contains("opposite side"));
        let p = profile("decoupling:\n  max_pin_distance_mm: 2.0\n");
        assert_eq!(decoupling_distance(&b, &p).len(), 1);
    }

    #[test]
    fn capacitance_parses_common_notations() {
        let close = |value: &str, want: f64| {
            let got = capacitance(value).unwrap();
            assert!((got - want).abs() < want * 1e-9, "{value}: {got}");
        };
        close("100nF", 100e-9);
        close("4u7", 4.7e-6);
        close("12pF", 12e-12);
        close("10µF/16V", 10e-6);
        assert_eq!(capacitance("DNP"), None);
    }

    #[test]
    fn decoupling_ignores_small_load_caps() {
        let b = board(
            r#"(footprint "IC" (layer "F.Cu") (at 0 0)
                 (property "Reference" "U1" (at 0 0) (layer "F.Fab"))
                 (pad "1" smd rect (at 0 0) (size 0.3 0.3) (layers "F.Cu") (net "XTAL")))
               (footprint "C" (layer "F.Cu") (at 9 0)
                 (property "Reference" "C1" (at 0 0) (layer "F.Fab"))
                 (property "Value" "12pF" (at 0 0) (layer "F.Fab"))
                 (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "F.Cu") (net "XTAL"))
                 (pad "2" smd rect (at 0 1) (size 0.5 0.5) (layers "F.Cu") (net "GND")))"#,
        );
        let p = profile("decoupling:\n  max_pin_distance_mm: 2.0\n");
        assert!(decoupling_distance(&b, &p).is_empty());
    }

    #[test]
    fn run_skips_superseded_audits_and_tags_profile_items() {
        let b = board(r#"(segment (start 0 0) (end 4 1) (width 0.2) (layer "F.Cu") (net "A"))"#);
        let all = run(&b, &Profile::default(), &HashSet::new());
        assert_eq!(all.len(), 1);
        assert_eq!(
            all[0].profile_item.as_deref(),
            Some("routing.octilinear_only")
        );
        let skip: HashSet<&str> = ["off_angle_track"].into();
        assert!(run(&b, &Profile::default(), &skip).is_empty());
    }
}
