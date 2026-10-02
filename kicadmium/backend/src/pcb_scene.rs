//! `/api/kicad/pcbscene`: the project's board reified into a
//! [`vector_view::Scene`] (kind `Pcb`, y down, millimetres, KiCad sheet frame).
//!
//! The board is parsed once by `pcb_view.rs` into the intermediate
//! [`PcbBoard`]; this module turns it into plain geometry so the browser only
//! needs the format-agnostic vector viewer:
//!
//! - one scene layer per KiCad layer, coloured and ordered like the KiCanvas
//!   view (bundled American Embedded dark theme, KiCanvas layer stack), plus
//!   synthetic drill layers (via walls, pad walls, pad holes, via holes,
//!   non-plated holes) and a hidden "Footprint bounds" pick layer;
//! - pads as filled polygons/circles per copper layer (every KiCad pad shape),
//!   drills and vias as `Prim::Hole`, tracks as polylines/arcs, zone fills as
//!   polygons, drawings with Béziers flattened, text as NewStroke strokes from
//!   `kicad_strokes`;
//! - nets, footprint and zone groups, and per-item properties.
//!
//! Layer `z` is the front-view paint order. The back view does not simply
//! reverse it (KiCanvas keeps each side's own stack), so the mirrored paint
//! order is listed in meta `back_order` (comma-separated layer ids). Within a
//! layer, items are emitted base-first: drawings/tracks/text, then pads, then
//! zones, which is KiCanvas's order inside a copper layer.
//!
//! THIRD-PARTY PROVENANCE: the pad outline geometry (`rect_outline`,
//! `stadium`, `shape_pieces`, `pad_pieces`, drill holes) is derived from
//! pastebom's `crates/viewer/src/render.rs` (github.com/meawoppl/pastebom.com,
//! same author), moved here from the former `frontend/src/pcb_view/geom.rs`.
//! Its author explicitly authorized the derived parts under kicadmium's MIT
//! license; see `docs/third-party.md`.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use kicad_strokes::{to_strokes, HJustify, TextSpec, VJustify};
use vector_view::style::{parse_css_color, Rgba};
use vector_view::{
    Group, GroupId, GroupKind, Item, Layer, LayerId, LayerKind, NetId, Prim, Prop, Role, Scene,
    SceneKind, Side,
};

use crate::pcb_model::*;
use crate::pcb_view::{build_board, copper_rank};
use crate::{
    current_source_revision, pick_project_file, rel, selected_project, AppError, AppState,
    ProjectContext, ProjectQuery,
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/kicad/pcbscene", get(endpoint))
}

/// Serialized scenes keyed by project id, reused while the revision holds.
type Cache = Mutex<HashMap<String, (String, Arc<Vec<u8>>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

async fn endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Response, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    let body = tokio::task::spawn_blocking(move || response_bytes(&project)).await??;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body.as_ref().clone(),
    )
        .into_response())
}

fn response_bytes(project: &ProjectContext) -> Result<Arc<Vec<u8>>> {
    let path = pick_project_file(project, "kicad_pcb")?
        .ok_or_else(|| anyhow!("project {} has no .kicad_pcb", project.id))?;
    // Read between two identical revisions so the scene matches the revision
    // it is labelled with (an editor save can land mid-read).
    for _ in 0..4 {
        let before = current_source_revision(project)?;
        if let Some((revision, body)) = cache().lock().unwrap().get(&project.id) {
            if *revision == before {
                return Ok(body.clone());
            }
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if current_source_revision(project)? != before {
            continue;
        }
        let filename = rel(&project.root, &path).unwrap_or_else(|_| path.display().to_string());
        let scene = build_scene(&text, &before, &filename)?;
        let body = Arc::new(serde_json::to_vec(&scene)?);
        cache()
            .lock()
            .unwrap()
            .insert(project.id.clone(), (before, body.clone()));
        return Ok(body);
    }
    Err(anyhow!("{} kept changing while being read", path.display()))
}

// -------------------------------------------------------------------- theme

const THEME_JSON: &str =
    include_str!("../../frontend/static/kicad-viewer/american-embedded-dark.json");

/// The KiCanvas board palette (`board` section of the bundled theme).
struct Theme {
    colors: HashMap<String, Rgba>,
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(THEME_JSON).unwrap_or_default();
        let mut colors = HashMap::new();
        let board = &json["board"];
        let mut read = |prefix: &str, map: &serde_json::Value| {
            for (k, v) in map.as_object().into_iter().flatten() {
                if let Some(c) = v.as_str().and_then(parse_css_color) {
                    colors.insert(format!("{prefix}{k}"), c);
                }
            }
        };
        read("", board);
        read("copper.", &board["copper"]);
        Theme { colors }
    })
}

fn alpha(c: Rgba, a: f64) -> Rgba {
    [c[0], c[1], c[2], (a * 255.0).round() as u8]
}

impl Theme {
    fn get(&self, key: &str, fallback: Rgba) -> Rgba {
        self.colors.get(key).copied().unwrap_or(fallback)
    }

    /// Layer colour as KiCanvas paints it (non-copper layers blend at 0.8).
    fn layer(&self, name: &str) -> Rgba {
        let key = match name.strip_suffix(".Cu") {
            Some(stem) => format!("copper.{}", stem.to_ascii_lowercase()),
            None => name.replace('.', "_").to_ascii_lowercase(),
        };
        let c = self.get(&key, [200, 200, 200, 255]);
        if name.ends_with(".Cu") {
            c
        } else {
            alpha(c, 0.8)
        }
    }
}

// --------------------------------------------------------------- layer stack

/// Synthetic layers (not KiCad layers): drill passes and the pick layer.
pub(crate) const VIA_WALLS: LayerId = 0xF000;
pub(crate) const PAD_WALLS: LayerId = 0xF001;
pub(crate) const PAD_HOLES: LayerId = 0xF002;
pub(crate) const VIA_HOLES: LayerId = 0xF003;
pub(crate) const NPTH: LayerId = 0xF004;
pub(crate) const FOOTPRINT_BOUNDS: LayerId = 0xF005;

/// Front-most-first drill passes, as in the KiCanvas stack.
const HOLE_STACK: [LayerId; 5] = [NPTH, VIA_HOLES, PAD_HOLES, PAD_WALLS, VIA_WALLS];

const SIDE: [&str; 7] = ["Cu", "Mask", "SilkS", "Adhes", "Paste", "CrtYd", "Fab"];
const TOP_DRAWING: [&str; 6] = [
    "Dwgs.User",
    "Cmts.User",
    "Eco1.User",
    "Eco2.User",
    "Edge.Cuts",
    "Margin",
];

#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Layer(String),
    Synthetic(LayerId),
}

fn user_rank(n: &str) -> usize {
    n.strip_prefix("User.")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX)
}

/// Front-most-first stack (KiCanvas `LayerSet` order); `flipped` puts the
/// back side on top.
fn stack(names: &[String], flipped: bool) -> Vec<Slot> {
    let (near, far) = if flipped { ("B", "F") } else { ("F", "B") };
    let has = |n: &str| names.iter().any(|x| x == n);
    let mut out = Vec::new();
    for n in TOP_DRAWING {
        if has(n) {
            out.push(Slot::Layer(n.to_string()));
        }
    }
    let mut users: Vec<&String> = names
        .iter()
        .filter(|n| {
            !TOP_DRAWING.contains(&n.as_str())
                && copper_rank(n).is_none()
                && !SIDE
                    .iter()
                    .any(|s| *n == &format!("F.{s}") || *n == &format!("B.{s}"))
        })
        .collect();
    users.sort_by_key(|n| user_rank(n));
    out.extend(users.into_iter().map(|n| Slot::Layer(n.clone())));
    out.extend(HOLE_STACK.iter().map(|id| Slot::Synthetic(*id)));
    let side = |out: &mut Vec<Slot>, prefix: &str| {
        for s in SIDE {
            let n = format!("{prefix}.{s}");
            if has(&n) {
                out.push(Slot::Layer(n));
            }
        }
    };
    side(&mut out, near);
    let mut inner: Vec<&String> = names
        .iter()
        .filter(|n| copper_rank(n).is_some_and(|r| r > 0 && r < 1000))
        .collect();
    inner.sort_by_key(|n| copper_rank(n));
    if flipped {
        inner.reverse();
    }
    out.extend(inner.into_iter().map(|n| Slot::Layer(n.clone())));
    side(&mut out, far);
    out
}

/// Paint order, back-most first.
fn paint_order(names: &[String], flipped: bool) -> Vec<Slot> {
    let mut s = stack(names, flipped);
    s.reverse();
    s
}

/// Layer-panel order (KiCanvas `in_ui_order`), then anything else.
fn ui_order(names: &[String]) -> Vec<String> {
    let mut copper: Vec<&String> = names.iter().filter(|n| copper_rank(n).is_some()).collect();
    copper.sort_by_key(|n| copper_rank(n));
    let fixed = [
        "F.Adhes",
        "B.Adhes",
        "F.Paste",
        "B.Paste",
        "F.SilkS",
        "B.SilkS",
        "F.Mask",
        "B.Mask",
        "Dwgs.User",
        "Cmts.User",
        "Eco1.User",
        "Eco2.User",
        "Edge.Cuts",
        "Margin",
        "F.CrtYd",
        "B.CrtYd",
        "F.Fab",
        "B.Fab",
    ];
    let mut out: Vec<String> = copper.into_iter().cloned().collect();
    for f in fixed {
        if names.iter().any(|n| n == f) {
            out.push(f.to_string());
        }
    }
    let mut rest: Vec<&String> = names.iter().filter(|n| !out.contains(n)).collect();
    rest.sort_by_key(|n| user_rank(n));
    out.extend(rest.into_iter().cloned());
    out
}

fn layer_kind(name: &str) -> LayerKind {
    let suffix = name.rsplit('.').next().unwrap_or("");
    match (name, suffix) {
        ("Edge.Cuts", _) => LayerKind::EdgeCuts,
        (_, "Cu") => LayerKind::Copper,
        (_, "SilkS") => LayerKind::Silkscreen,
        (_, "Mask") => LayerKind::SolderMask,
        (_, "Paste") => LayerKind::Paste,
        (_, "Fab") => LayerKind::Fabrication,
        (_, "CrtYd") => LayerKind::Courtyard,
        (_, "Adhes") => LayerKind::Other,
        _ => LayerKind::Drawing,
    }
}

fn layer_side(name: &str) -> Side {
    if name.starts_with("F.") {
        Side::Front
    } else if name.starts_with("B.") {
        Side::Back
    } else if copper_rank(name).is_some() {
        Side::Inner
    } else {
        Side::None
    }
}

// ------------------------------------------------------------ pad geometry

fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

fn rp(p: Pt) -> Pt {
    [r4(p[0]), r4(p[1])]
}

/// KiCad rotation: counter-clockwise on screen (y down).
fn rotate(p: Pt, deg: f64) -> Pt {
    if deg == 0.0 {
        return p;
    }
    let (s, c) = deg.to_radians().sin_cos();
    [p[0] * c + p[1] * s, -p[0] * s + p[1] * c]
}

/// Pad-frame to board transform (hole at `pos`, shape at `offset`).
fn pad_to_board(pad: &PcbPad, q: Pt) -> Pt {
    let r = rotate([q[0] + pad.offset[0], q[1] + pad.offset[1]], pad.angle);
    rp([pad.pos[0] + r[0], pad.pos[1] + r[1]])
}

/// Filled or stroked piece of a pad outline in board coordinates.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Circle(Pt, f64),
    Polygon(Vec<Pt>),
    /// Stroked centre line of `width` (custom-pad line/arc primitives).
    Stroke(Vec<Pt>, f64),
}

impl Piece {
    fn into_prim(self) -> Prim {
        match self {
            Piece::Circle(center, radius) => Prim::Circle {
                center,
                radius,
                fill: true,
                stroke: 0.0,
            },
            Piece::Polygon(outer) => Prim::Polygon {
                outer,
                holes: Vec::new(),
                fill: true,
                stroke: 0.0,
            },
            Piece::Stroke(points, width) => Prim::Polyline { points, width },
        }
    }
}

fn arc_pts(c: Pt, r: f64, a0: f64, a1: f64, steps: usize, out: &mut Vec<Pt>) {
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f64 / steps as f64;
        out.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
    }
}

/// Rounded/chamfered rectangle outline centred on the origin (pad frame,
/// increasing-angle order). `chamfer` bits: 1 TL, 2 TR, 4 BL, 8 BR.
fn rect_outline(size: Pt, radius: f64, chamfer: u8, chamfer_len: f64) -> Vec<Pt> {
    let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
    let r = radius.clamp(0.0, hx.min(hy));
    let c = chamfer_len.clamp(0.0, hx.min(hy));
    let mut out = Vec::new();
    // Corners in increasing-angle (clockwise on screen) order:
    // top-left, top-right, bottom-right, bottom-left.
    let corners: [(Pt, u8, f64); 4] = [
        ([-hx, -hy], 1, PI),
        ([hx, -hy], 2, 1.5 * PI),
        ([hx, hy], 8, 0.0),
        ([-hx, hy], 4, 0.5 * PI),
    ];
    for (corner, bit, start) in corners {
        let sx = corner[0].signum();
        let sy = corner[1].signum();
        if chamfer & bit != 0 && c > 0.0 {
            // Cut from the incoming edge to the outgoing edge.
            let (p1, p2) = match bit {
                1 => ([-hx, -hy + c], [-hx + c, -hy]),
                2 => ([hx - c, -hy], [hx, -hy + c]),
                8 => ([hx, hy - c], [hx - c, hy]),
                _ => ([-hx + c, hy], [-hx, hy - c]),
            };
            out.push(p1);
            out.push(p2);
        } else if r > 0.0 {
            let center = [corner[0] - sx * r, corner[1] - sy * r];
            arc_pts(center, r, start, start + PI / 2.0, 8, &mut out);
        } else {
            out.push(corner);
        }
    }
    out
}

fn stadium(size: Pt) -> Vec<Pt> {
    let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
    let mut out = Vec::new();
    if hx >= hy {
        let d = hx - hy;
        arc_pts([d, 0.0], hy, -PI / 2.0, PI / 2.0, 16, &mut out);
        arc_pts([-d, 0.0], hy, PI / 2.0, 1.5 * PI, 16, &mut out);
    } else {
        let d = hy - hx;
        arc_pts([0.0, d], hx, 0.0, PI, 16, &mut out);
        arc_pts([0.0, -d], hx, PI, TAU, 16, &mut out);
    }
    out
}

/// Custom-pad primitive outline pieces under `map` (pad frame to board).
fn graphic_pieces(g: &PcbGraphic, map: &dyn Fn(Pt) -> Pt) -> Vec<Piece> {
    let mut out = Vec::new();
    match &g.shape {
        PcbShape::Segment { a, b } => out.push(Piece::Stroke(vec![map(*a), map(*b)], g.width)),
        PcbShape::Arc { c, r, start, end } => {
            let mut pts = Vec::new();
            let steps = ((end - start).abs() / TAU * 64.0).ceil().max(4.0) as usize;
            arc_pts(*c, *r, *start, *end, steps, &mut pts);
            out.push(Piece::Stroke(pts.into_iter().map(map).collect(), g.width));
        }
        PcbShape::Circle { c, r } => {
            if g.filled {
                out.push(Piece::Circle(map(*c), r + g.width / 2.0));
            } else {
                let mut pts = Vec::new();
                arc_pts(*c, *r, 0.0, TAU, 64, &mut pts);
                out.push(Piece::Stroke(pts.into_iter().map(map).collect(), g.width));
            }
        }
        PcbShape::Polygon { pts } => {
            let mapped: Vec<Pt> = pts.iter().map(|p| map(*p)).collect();
            if g.filled {
                out.push(Piece::Polygon(mapped.clone()));
            }
            if g.width > 0.0 {
                let mut ring = mapped;
                if let Some(first) = ring.first().copied() {
                    ring.push(first);
                }
                out.push(Piece::Stroke(ring, g.width));
            }
        }
        PcbShape::Polyline { pts } => out.push(Piece::Stroke(
            pts.iter().map(|p| map(*p)).collect(),
            g.width,
        )),
    }
    out
}

fn shape_pieces(pad: &PcbPad, shape: &str, size: Pt) -> Vec<Piece> {
    let map = |q: Pt| pad_to_board(pad, q);
    let min = size[0].min(size[1]);
    let poly = |pts: Vec<Pt>| Piece::Polygon(pts.into_iter().map(map).collect());
    match shape {
        "circle" => vec![Piece::Circle(map([0.0, 0.0]), size[0] / 2.0)],
        "oval" => {
            if (size[0] - size[1]).abs() < 1e-9 {
                vec![Piece::Circle(map([0.0, 0.0]), size[0] / 2.0)]
            } else {
                vec![poly(stadium(size))]
            }
        }
        "roundrect" => vec![poly(rect_outline(
            size,
            min * pad.rratio,
            pad.chamfer,
            min * pad.chamfer_ratio,
        ))],
        "trapezoid" => {
            let (px, py) = (size[0] / 2.0, size[1] / 2.0);
            let (dx, dy) = (pad.delta[0] / 2.0, pad.delta[1] / 2.0);
            // KiCad's corner order (KiCanvas PadPainter), drawn increasing-angle.
            vec![poly(vec![
                [-px + dy, -py - dx],
                [px - dy, -py + dx],
                [px + dy, py - dx],
                [-px - dy, py + dx],
            ])]
        }
        // "rect" and anything unknown.
        _ => vec![poly(rect_outline(
            size,
            0.0,
            pad.chamfer,
            min * pad.chamfer_ratio,
        ))],
    }
}

/// Copper outline of `pad` in board coordinates.
fn pad_pieces(pad: &PcbPad) -> Vec<Piece> {
    if pad.shape != "custom" {
        return shape_pieces(pad, &pad.shape, pad.size);
    }
    let mut out = shape_pieces(pad, &pad.custom_anchor, pad.size);
    let map = |q: Pt| pad_to_board(pad, q);
    for prim in &pad.custom {
        out.extend(graphic_pieces(prim, &map));
    }
    out
}

/// Drill hole (round, or an oblong slot rotated with the pad).
fn drill_prim(pad: &PcbPad) -> Option<Prim> {
    let drill = pad.drill.as_ref()?;
    let size = if drill.oval {
        drill.size
    } else {
        [drill.size[0], drill.size[0]]
    };
    Some(Prim::Hole {
        center: pad.pos,
        size,
        // KiCad degrees (counter-clockwise on a y-down screen) to scene
        // radians (from +x toward +y).
        rotation: -pad.angle.to_radians(),
    })
}

/// Drawing shape as a scene primitive.
fn graphic_prim(g: &PcbGraphic) -> Prim {
    match &g.shape {
        PcbShape::Segment { a, b } => Prim::Polyline {
            points: vec![*a, *b],
            width: g.width,
        },
        PcbShape::Arc { c, r, start, end } => Prim::Arc {
            center: *c,
            radius: *r,
            start: *start,
            end: *end,
            width: g.width,
        },
        PcbShape::Circle { c, r } => Prim::Circle {
            center: *c,
            radius: *r,
            fill: g.filled,
            stroke: g.width,
        },
        PcbShape::Polygon { pts } => Prim::Polygon {
            outer: pts.clone(),
            holes: Vec::new(),
            fill: g.filled,
            stroke: g.width,
        },
        PcbShape::Polyline { pts } => Prim::Polyline {
            points: pts.clone(),
            width: g.width,
        },
    }
}

/// KiCad text through the NewStroke layout.
fn text_prim(t: &PcbText) -> Prim {
    let justify_h = match t.h_align.as_str() {
        "left" => HJustify::Left,
        "right" => HJustify::Right,
        _ => HJustify::Center,
    };
    let justify_v = match t.v_align.as_str() {
        "top" => VJustify::Top,
        "bottom" => VJustify::Bottom,
        _ => VJustify::Center,
    };
    to_strokes(&TextSpec {
        text: t.text.clone(),
        pos: t.pos,
        size: t.size,
        thickness: t.thickness,
        // Keep-upright is already folded into `angle` by the parser.
        angle_deg: t.angle,
        justify_h,
        justify_v,
        mirror: t.mirrored,
        italic: t.italic,
        bold: t.bold,
        // kicad-cli ignores `(line_spacing)` when plotting boards; match the plots.
        line_spacing: 1.0,
        keep_upright: false,
        y_down: true,
    })
    .into_prim()
}

// ------------------------------------------------------------------ builder

fn mm(v: f64) -> String {
    let s = format!("{v:.3}");
    format!("{} mm", s.trim_end_matches('0').trim_end_matches('.'))
}

struct Builder<'a> {
    board: &'a PcbBoard,
    scene: Scene,
    next: u32,
}

impl Builder<'_> {
    fn add(
        &mut self,
        layer: LayerId,
        role: Role,
        prim: Prim,
        net: u32,
        group: Option<GroupId>,
        props: Vec<Prop>,
    ) {
        self.next += 1;
        self.scene.items.push(Item {
            id: self.next,
            layer,
            role,
            prim,
            net: (net != 0).then_some(net as NetId),
            group,
            props,
        });
    }

    fn layer_name(&self, l: u16) -> &str {
        self.board
            .layers
            .get(l as usize)
            .map_or("", |l| l.name.as_str())
    }

    fn layer_names(&self, layers: &[u16]) -> String {
        layers
            .iter()
            .map(|l| self.layer_name(*l))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn net_name(&self, net: u32) -> String {
        match self.board.nets.get(net as usize) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => "(no net)".into(),
        }
    }

    fn graphic(&mut self, g: &PcbGraphic, group: Option<GroupId>) {
        let role = if self.layer_name(g.layer) == "Edge.Cuts" {
            Role::Outline
        } else {
            Role::Graphic
        };
        self.add(g.layer, role, graphic_prim(g), 0, group, Vec::new());
    }

    fn text(&mut self, t: &PcbText, group: Option<GroupId>) {
        if !t.visible {
            return;
        }
        let props = vec![Prop::new("kind", &t.kind), Prop::new("text", &t.text)];
        self.add(t.layer, Role::Text, text_prim(t), 0, group, props);
    }

    fn pad_props(&self, pad: &PcbPad) -> Vec<Prop> {
        let mut props = vec![
            Prop::new("Pad", &pad.number),
            Prop::new("Net", self.net_name(pad.net)),
            Prop::new("Pad type", format!("{} {}", pad.kind, pad.shape)),
            Prop::new("Size", format!("{} × {}", mm(pad.size[0]), mm(pad.size[1]))),
        ];
        if let Some(d) = &pad.drill {
            let drill = if d.oval {
                format!("{} × {}", mm(d.size[0]), mm(d.size[1]))
            } else {
                mm(d.size[0])
            };
            props.push(Prop::new("Drill", drill));
        }
        if !pad.pin_function.is_empty() {
            props.push(Prop::new("Pin function", &pad.pin_function));
        }
        props.push(Prop::new("Pad layers", self.layer_names(&pad.layers)));
        props
    }

    fn footprint(&mut self, fp: &PcbFootprint) {
        let mut props = vec![
            Prop::new("Reference", &fp.reference),
            Prop::new("Value", &fp.value),
            Prop::new("Footprint", &fp.footprint),
            Prop::new("Layer", self.layer_name(fp.layer)),
            Prop::new(
                "Position",
                format!("{:.3}, {:.3} mm, {}°", fp.pos[0], fp.pos[1], fp.angle),
            ),
        ];
        if !fp.attr.is_empty() {
            props.push(Prop::new("Type", &fp.attr));
        }
        if fp.dnp {
            props.push(Prop::new("DNP", "yes"));
        }
        if !fp.description.is_empty() {
            props.push(Prop::new("Description", &fp.description));
        }
        for (k, v) in fp.properties.iter().filter(|(_, v)| !v.is_empty()) {
            props.push(Prop::new(k, v));
        }
        let id = self.scene.groups.len() as GroupId + 1;
        self.scene.groups.push(Group {
            id,
            kind: GroupKind::Footprint,
            label: fp.reference.clone(),
            props,
        });
        let group = Some(id);
        for g in &fp.graphics {
            self.graphic(g, group);
        }
        for t in &fp.texts {
            self.text(t, group);
        }
        for pad in &fp.pads {
            let pieces = pad_pieces(pad);
            let props = self.pad_props(pad);
            // Copper only: like KiCanvas, mask/paste apertures are not painted.
            let copper: Vec<u16> = pad
                .layers
                .iter()
                .copied()
                .filter(|l| self.layer_name(*l).ends_with(".Cu"))
                .collect();
            let mut targets: Vec<LayerId> = copper;
            if pad.kind == "thru_hole" {
                targets.push(PAD_WALLS);
            }
            for layer in targets {
                for piece in &pieces {
                    self.add(
                        layer,
                        Role::Pad,
                        piece.clone().into_prim(),
                        pad.net,
                        group,
                        props.clone(),
                    );
                }
            }
            let hole_layer = match pad.kind.as_str() {
                "thru_hole" => Some(PAD_HOLES),
                "np_thru_hole" => Some(NPTH),
                _ => None,
            };
            if let (Some(layer), Some(hole)) = (hole_layer, drill_prim(pad)) {
                self.add(layer, Role::Hole, hole, pad.net, group, props.clone());
            }
        }
        self.add(
            FOOTPRINT_BOUNDS,
            Role::Other,
            Prim::Polygon {
                outer: fp.bbox.clone(),
                holes: Vec::new(),
                fill: true,
                stroke: 0.0,
            },
            0,
            group,
            Vec::new(),
        );
    }

    fn tracks(&mut self) {
        for t in &self.board.tracks {
            let (kind, length, prim) = match &t.shape {
                PcbShape::Segment { a, b } => (
                    "Track",
                    (b[0] - a[0]).hypot(b[1] - a[1]),
                    Prim::Polyline {
                        points: vec![*a, *b],
                        width: t.width,
                    },
                ),
                PcbShape::Arc { c, r, start, end } => (
                    "Arc track",
                    r * (end - start).abs(),
                    Prim::Arc {
                        center: *c,
                        radius: *r,
                        start: *start,
                        end: *end,
                        width: t.width,
                    },
                ),
                other => (
                    "Track",
                    0.0,
                    graphic_prim(&PcbGraphic {
                        layer: t.layer,
                        shape: other.clone(),
                        width: t.width,
                        filled: false,
                    }),
                ),
            };
            let props = vec![
                Prop::new("Type", kind),
                Prop::new("Net", self.net_name(t.net)),
                Prop::new("Layer", self.layer_name(t.layer)),
                Prop::new("Width", mm(t.width)),
                Prop::new("Length", mm(length)),
            ];
            self.add(t.layer, Role::Track, prim, t.net, None, props);
        }
    }

    fn vias(&mut self) {
        for v in &self.board.vias {
            let props = vec![
                Prop::new("Type", format!("Via ({})", v.kind)),
                Prop::new("Net", self.net_name(v.net)),
                Prop::new("Size", mm(v.size)),
                Prop::new("Drill", mm(v.drill)),
                Prop::new("Layers", self.layer_names(&v.layers)),
            ];
            let hole = |d: f64| Prim::Hole {
                center: v.pos,
                size: [d, d],
                rotation: 0.0,
            };
            self.add(
                VIA_WALLS,
                Role::Via,
                hole(v.size),
                v.net,
                None,
                props.clone(),
            );
            self.add(VIA_HOLES, Role::Hole, hole(v.drill), v.net, None, props);
        }
    }

    fn zones(&mut self) {
        for z in self.board.zones.iter().filter(|z| !z.keepout) {
            let mut props = vec![
                Prop::new("Type", "Zone"),
                Prop::new("Net", self.net_name(z.net)),
                Prop::new("Layers", self.layer_names(&z.layers)),
                Prop::new("Priority", z.priority.to_string()),
            ];
            if !z.name.is_empty() {
                props.insert(0, Prop::new("Name", &z.name));
            }
            let id = self.scene.groups.len() as GroupId + 1;
            let label = if z.name.is_empty() {
                format!("Zone {}", self.net_name(z.net))
            } else {
                z.name.clone()
            };
            self.scene.groups.push(Group {
                id,
                kind: GroupKind::Zone,
                label,
                props: props.clone(),
            });
            for f in &z.fills {
                let prim = Prim::Polygon {
                    outer: f.pts.clone(),
                    holes: Vec::new(),
                    fill: true,
                    stroke: f.stroke,
                };
                self.add(f.layer, Role::Zone, prim, z.net, Some(id), props.clone());
            }
        }
    }

    fn layers(&mut self) {
        let t = theme();
        let names: Vec<String> = self.board.layers.iter().map(|l| l.name.clone()).collect();
        let front = paint_order(&names, false);
        let id_of = |slot: &Slot| match slot {
            Slot::Layer(n) => names.iter().position(|x| x == n).map(|i| i as LayerId),
            Slot::Synthetic(id) => Some(*id),
        };
        let mut z: HashMap<LayerId, i32> = HashMap::new();
        for (i, slot) in front.iter().enumerate() {
            if let Some(id) = id_of(slot) {
                z.insert(id, i as i32 * 10);
            }
        }
        let top = front.len() as i32 * 10;
        for name in ui_order(&names) {
            let id = names.iter().position(|x| *x == name).unwrap() as LayerId;
            let layer = &self.board.layers[id as usize];
            self.scene.layers.push(Layer {
                id,
                name: name.clone(),
                kind: layer_kind(&name),
                side: layer_side(&name),
                // Layers outside the stack (unknown names) draw on top.
                z: z.get(&id).copied().unwrap_or(top),
                color: t.layer(&name),
                visible: !layer.disabled,
            });
            if !layer.alias.is_empty() && layer.alias != name {
                self.scene
                    .meta
                    .push(Prop::new(format!("layer_alias:{name}"), &layer.alias));
            }
        }
        let background = t.get("background", [30, 30, 30, 255]);
        let synthetic = [
            (
                VIA_WALLS,
                "Via walls",
                alpha(t.get("via_through", [200, 200, 200, 255]), 0.8),
            ),
            (
                PAD_WALLS,
                "Pad walls",
                alpha(t.get("pad_through_hole", [143, 188, 187, 191]), 0.8),
            ),
            (PAD_HOLES, "Pad holes", alpha(background, 0.8)),
            (
                VIA_HOLES,
                "Via holes",
                alpha(t.get("via_hole", [140, 115, 80, 204]), 0.8),
            ),
            (
                NPTH,
                "Non-plated holes",
                alpha(t.get("non_plated_hole", [26, 196, 210, 255]), 0.8),
            ),
        ];
        for (id, name, color) in synthetic {
            self.scene.layers.push(Layer {
                id,
                name: name.into(),
                kind: LayerKind::Drill,
                side: Side::None,
                z: z[&id],
                color,
                visible: true,
            });
        }
        self.scene.layers.push(Layer {
            id: FOOTPRINT_BOUNDS,
            name: "Footprint bounds".into(),
            kind: LayerKind::Overlay,
            side: Side::None,
            z: top + 10,
            color: [64, 169, 255, 255],
            visible: false,
        });
        let back: Vec<String> = paint_order(&names, true)
            .iter()
            .filter_map(id_of)
            .map(|id| id.to_string())
            .collect();
        self.scene.meta.extend([
            Prop::new("back_order", back.join(",")),
            Prop::new("background", vector_view::style::css_rgba(background, 1.0)),
            Prop::new(
                "grid",
                vector_view::style::css_rgba(t.get("grid", [50, 50, 50, 255]), 1.0),
            ),
        ]);
    }
}

/// Reify `.kicad_pcb` text into a scene labelled with `revision`.
pub(crate) fn build_scene(text: &str, revision: &str, filename: &str) -> Result<Scene> {
    let board = build_board(text)?;
    Ok(scene_from_board(&board, revision, filename))
}

pub(crate) fn scene_from_board(board: &PcbBoard, revision: &str, filename: &str) -> Scene {
    let mut b = Builder {
        board,
        scene: Scene::new(SceneKind::Pcb, true),
        next: 0,
    };
    b.scene.meta.extend([
        Prop::new("revision", revision),
        Prop::new("filename", filename),
        Prop::new("title", &board.title),
    ]);
    b.layers();
    b.scene.nets = board
        .nets
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, name)| vector_view::Net {
            id: i as NetId,
            name: name.clone(),
        })
        .collect();
    // Base items first (drawings, text, tracks), then pads, then zones: the
    // KiCanvas order within a layer. Per-layer item order is the draw order.
    for g in &board.graphics {
        b.graphic(g, None);
    }
    for t in &board.texts {
        b.text(t, None);
    }
    b.tracks();
    for fp in &board.footprints {
        b.footprint(fp);
    }
    b.vias();
    b.zones();
    // Stable within-layer order: base, pads, zones.
    let rank = |r: Role| match r {
        Role::Zone => 2,
        Role::Pad => 1,
        _ => 0,
    };
    b.scene.items.sort_by_key(|i| rank(i.role));
    b.scene.recompute_bbox();
    for w in &board.warnings {
        b.scene.meta.push(Prop::new("warning", w));
    }
    b.scene
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (36 "B.SilkS" user "B.Silkscreen")
          (37 "F.SilkS" user "F.Silkscreen") (44 "Edge.Cuts" user))
  (net 0 "") (net 1 "GND") (net 2 "VCC")
  (gr_rect (start 100 100) (end 140 120) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts"))
  (gr_text "HELLO" (at 120 118 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)) (justify left)))
  (footprint "Resistor_SMD:R_0805" (layer "F.Cu") (at 110 110 90)
    (property "Reference" "R1" (at 0 -1.5 90) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "10k" (at 0 1.5 90) (layer "F.Fab") (hide yes) (effects (font (size 1 1) (thickness 0.15))))
    (fp_line (start -1 -0.7) (end 1 -0.7) (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
    (pad "1" smd roundrect (at -0.9 0 90) (size 1 1.4) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25) (net 1 "GND"))
    (pad "2" smd rect (at 0.9 0 90) (size 1 1.4) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "VCC"))
  )
  (footprint "Conn:TH" (layer "F.Cu") (at 130 110)
    (fp_text reference "J1" (at 0 -2) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
    (pad "1" thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "GND"))
    (pad "2" thru_hole oval (at 2.54 0 90) (size 1.7 2.2) (drill oval 0.8 1.2) (layers "*.Cu" "*.Mask") (net 2 "VCC"))
  )
  (segment (start 110 109.1) (end 130 110) (width 0.25) (layer "F.Cu") (net 1))
  (arc (start 120 105) (mid 122 103) (end 124 105) (width 0.2) (layer "B.Cu") (net 2))
  (via (at 125 105) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2))
  (zone (net 1) (net_name "GND") (layer "B.Cu") (hatch edge 0.5)
    (connect_pads (clearance 0.2)) (min_thickness 0.25) (filled_areas_thickness no)
    (fill yes (thermal_gap 0.5) (thermal_bridge_width 0.5))
    (polygon (pts (xy 100 100) (xy 140 100) (xy 140 120) (xy 100 120)))
    (filled_polygon (layer "B.Cu") (pts (xy 101 101) (xy 139 101) (xy 139 119) (xy 101 119))))
)"#;

    fn scene() -> Scene {
        build_scene(BOARD, "rev1", "board.kicad_pcb").unwrap()
    }

    fn layer_id(s: &Scene, name: &str) -> LayerId {
        s.layers.iter().find(|l| l.name == name).unwrap().id
    }

    fn meta<'a>(s: &'a Scene, key: &str) -> &'a str {
        &s.meta.iter().find(|p| p.key == key).unwrap().value
    }

    #[test]
    fn layers_follow_kicanvas_stack_and_theme() {
        let s = scene();
        assert_eq!(s.kind, SceneKind::Pcb);
        assert!(s.y_down);
        assert_eq!(meta(&s, "revision"), "rev1");
        let z = |n: &str| s.layer(layer_id(&s, n)).unwrap().z;
        // Front view: back side under front copper, drawings on top.
        assert!(z("B.SilkS") < z("B.Cu") && z("B.Cu") < z("F.SilkS"));
        assert!(z("F.SilkS") < z("F.Cu") && z("F.Cu") < z("Edge.Cuts"));
        assert!(z("F.Cu") < s.layer(VIA_WALLS).unwrap().z);
        let fcu = s.layer(layer_id(&s, "F.Cu")).unwrap();
        assert_eq!(fcu.side, Side::Front);
        assert_eq!(fcu.kind, LayerKind::Copper);
        assert_eq!(fcu.color[3], 191, "copper keeps the theme alpha");
        assert_eq!(s.layer(layer_id(&s, "F.SilkS")).unwrap().color[3], 204);
        assert_eq!(meta(&s, "layer_alias:B.SilkS"), "B.Silkscreen");
        // Back view lists F.Cu before (under) B.Cu.
        let back: Vec<LayerId> = meta(&s, "back_order")
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        let pos = |id| back.iter().position(|x| *x == id).unwrap();
        assert!(pos(layer_id(&s, "F.Cu")) < pos(layer_id(&s, "B.Cu")));
        assert!(pos(layer_id(&s, "B.SilkS")) < pos(layer_id(&s, "Edge.Cuts")));
        assert!(!s.layer(FOOTPRINT_BOUNDS).unwrap().visible);
    }

    #[test]
    fn items_carry_roles_nets_groups_and_props() {
        let s = scene();
        let fcu = layer_id(&s, "F.Cu");
        let bcu = layer_id(&s, "B.Cu");
        let gnd = s.nets.iter().find(|n| n.name == "GND").unwrap().id;
        let count = |role: Role, layer: LayerId| {
            s.items
                .iter()
                .filter(|i| i.role == role && i.layer == layer)
                .count()
        };
        // R1's two SMD pads plus J1's two THT pads on F.Cu; THT also on B.Cu
        // and the pad-wall pass.
        assert_eq!(count(Role::Pad, fcu), 4);
        assert_eq!(count(Role::Pad, bcu), 2);
        assert_eq!(count(Role::Pad, PAD_WALLS), 2);
        assert_eq!(count(Role::Hole, PAD_HOLES), 2);
        assert_eq!(count(Role::Via, VIA_WALLS), 1);
        assert_eq!(count(Role::Hole, VIA_HOLES), 1);
        assert_eq!(count(Role::Zone, bcu), 1);
        assert_eq!(count(Role::Track, fcu), 1);
        assert_eq!(count(Role::Outline, layer_id(&s, "Edge.Cuts")), 1);

        let r1 = s.groups.iter().find(|g| g.label == "R1").unwrap();
        assert_eq!(r1.kind, GroupKind::Footprint);
        assert!(r1.props.contains(&Prop::new("Value", "10k")));
        let pad1 = s
            .items
            .iter()
            .find(|i| i.role == Role::Pad && i.group == Some(r1.id))
            .unwrap();
        assert_eq!(pad1.net, Some(gnd));
        assert!(pad1.props.contains(&Prop::new("Pad", "1")));
        assert!(pad1.props.contains(&Prop::new("Pad type", "smd roundrect")));
        // Pad at local (-0.9, 0) with the footprint at 90 degrees: (110, 110.9).
        assert!(vector_view::hit::prim_contains(
            &pad1.prim,
            [110.0, 110.9],
            0.0
        ));
        // The oval slot follows the pad's 90-degree rotation.
        let slot = s
            .items
            .iter()
            .find(|i| {
                i.layer == PAD_HOLES && i.props.contains(&Prop::new("Drill", "0.8 mm × 1.2 mm"))
            })
            .unwrap();
        match slot.prim {
            Prim::Hole { size, rotation, .. } => {
                assert_eq!(size, [0.8, 1.2]);
                assert!((rotation + PI / 2.0).abs() < 1e-9);
            }
            ref other => panic!("{other:?}"),
        }
        // The hidden value text is omitted; the reference is kept.
        let texts: Vec<&str> = s
            .items
            .iter()
            .filter(|i| i.role == Role::Text)
            .flat_map(|i| i.props.iter().filter(|p| p.key == "kind"))
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(texts.len(), 3);
        assert!(!texts.contains(&"value"));
        // Arc tracks are CCW in scene axes (through the top, y = 103).
        let arc = s.items.iter().find(|i| matches!(i.prim, Prim::Arc { .. }));
        match &arc.unwrap().prim {
            Prim::Arc {
                center,
                start,
                end,
                ..
            } => {
                assert!((center[0] - 122.0).abs() < 1e-6);
                let mid = -PI / 2.0;
                assert!((mid - start).rem_euclid(TAU) <= end - start);
            }
            _ => unreachable!(),
        }
        // Within a layer: tracks, then pads, then zones.
        let order: Vec<Role> = s
            .items
            .iter()
            .filter(|i| i.layer == bcu)
            .map(|i| i.role)
            .collect();
        assert_eq!(order.last(), Some(&Role::Zone));
        assert!(s.groups.iter().any(|g| g.kind == GroupKind::Zone));
        let json = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items.len(), s.items.len());
    }

    #[test]
    fn pad_outlines() {
        let o = rect_outline([2.0, 1.0], 0.25, 0, 0.0);
        assert!(o.len() > 4);
        let ch = rect_outline([2.0, 2.0], 0.0, 1 | 8, 0.5);
        assert!(!vector_view::hit::polygon_contains(&ch, &[], [-0.95, -0.95]));
        assert!(vector_view::hit::polygon_contains(&ch, &[], [0.95, -0.95]));
        let pad = PcbPad {
            shape: "oval".into(),
            size: [1.7, 2.2],
            offset: [0.0, 0.1],
            pos: [10.0, 10.0],
            ..PcbPad::default()
        };
        let prims: Vec<Prim> = pad_pieces(&pad).into_iter().map(Piece::into_prim).collect();
        let hit = |p| {
            prims
                .iter()
                .any(|x| vector_view::hit::prim_contains(x, p, 0.0))
        };
        assert!(hit([10.0, 11.15]) && !hit([10.0, 8.85]));
        let trap = PcbPad {
            shape: "trapezoid".into(),
            size: [2.0, 1.0],
            delta: [0.0, 0.5],
            ..PcbPad::default()
        };
        assert!(matches!(&pad_pieces(&trap)[0], Piece::Polygon(p) if p.len() == 4));
    }
}
