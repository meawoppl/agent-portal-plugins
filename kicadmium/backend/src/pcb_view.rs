//! `/api/kicad/pcbview`: the project's board as a render-ready
//! [`shared::pcb::PcbBoard`] for the Rust PCB view.
//!
//! Parsing goes through `kct` only: [`kct::schema::pcb::Pcb`] supplies the
//! layer table, nets, copper (tracks, arcs, vias, zones) and footprint
//! metadata; footprint bodies, board graphics and text are read from the
//! same parsed `kct::sexp` tree for the attributes the typed model does not
//! carry (pad chamfers and primitives, text effects, Béziers). All
//! output is in KiCad's sheet frame (the typed model's board-relative
//! coordinates get the board origin added back).

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use kct::schema::pcb::{arc_points_from_sexp, fill_token_is_filled, is_footprint_tag, Pcb};
use kct::sexp::{SExp, Value};
use shared::pcb::*;

use crate::{
    current_source_revision, pick_project_file, rel, selected_project, AppError, AppState,
    ProjectContext, ProjectQuery,
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/kicad/pcbview", get(endpoint))
}

/// Serialized responses keyed by project id, reused while the revision holds.
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
    // Read between two identical revisions so the board matches the revision
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
        let response = PcbViewResponse {
            ok: true,
            revision: before.clone(),
            filename: rel(&project.root, &path).unwrap_or_else(|_| path.display().to_string()),
            board: build_board(&text)?,
        };
        let body = Arc::new(serde_json::to_vec(&response)?);
        cache()
            .lock()
            .unwrap()
            .insert(project.id.clone(), (before, body.clone()));
        return Ok(body);
    }
    Err(anyhow!("{} kept changing while being read", path.display()))
}

// ------------------------------------------------------------------ helpers

fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

fn rp(p: Pt) -> Pt {
    [r4(p[0]), r4(p[1])]
}

fn fnum(node: &SExp, index: usize) -> Option<f64> {
    node.float_at(index)
}

fn xy_of(node: &SExp) -> Pt {
    [fnum(node, 0).unwrap_or(0.0), fnum(node, 1).unwrap_or(0.0)]
}

fn child_xy(node: &SExp, tag: &str) -> Option<Pt> {
    node.get(tag).map(xy_of)
}

/// Text of the `index`-th atom (numbers as written).
fn atom_text(node: &SExp, index: usize) -> Option<String> {
    let child = node.children.get(index)?;
    child.is_atom().then(|| node.text_at(index)).flatten()
}

fn has_atom(node: &SExp, token: &str) -> bool {
    node.children
        .iter()
        .any(|c| c.is_atom() && c.value.as_ref().and_then(Value::as_str) == Some(token))
}

/// `(tag yes)` / bare `tag` atom.
fn yes(node: &SExp, tag: &str) -> bool {
    has_atom(node, tag)
        || node
            .get(tag)
            .is_some_and(|n| matches!(n.string_at(0), None | Some("yes" | "true")))
}

/// Rotate `p` by `deg` counter-clockwise on a y-down screen (KiCad sense).
fn rotate(p: Pt, deg: f64) -> Pt {
    if deg == 0.0 {
        return p;
    }
    let (s, c) = deg.to_radians().sin_cos();
    [p[0] * c + p[1] * s, -p[0] * s + p[1] * c]
}

/// Footprint-local to board transform.
#[derive(Clone, Copy)]
struct Xform {
    origin: Pt,
    angle: f64,
}

impl Xform {
    const IDENTITY: Xform = Xform {
        origin: [0.0, 0.0],
        angle: 0.0,
    };

    fn apply(&self, p: Pt) -> Pt {
        let q = rotate(p, self.angle);
        rp([q[0] + self.origin[0], q[1] + self.origin[1]])
    }
}

/// Circle through three points as an increasing-angle arc from `a` to `c`
/// via `b`; `None` when collinear.
fn arc3(a: Pt, b: Pt, c: Pt) -> Option<PcbShape> {
    let (bx, by) = (b[0] - a[0], b[1] - a[1]);
    let (cx, cy) = (c[0] - a[0], c[1] - a[1]);
    let det = 2.0 * (bx * cy - by * cx);
    let scale = (bx * bx + by * by).max(cx * cx + cy * cy);
    if scale == 0.0 || det.abs() <= 1e-12 * scale {
        return None;
    }
    let ux = ((bx * bx + by * by) * cy - (cx * cx + cy * cy) * by) / det;
    let uy = (bx * (cx * cx + cy * cy) - cx * (bx * bx + by * by)) / det;
    let center = [a[0] + ux, a[1] + uy];
    let r = ux.hypot(uy);
    let ang = |p: Pt| (p[1] - center[1]).atan2(p[0] - center[0]);
    let (sa, ma, ea) = (ang(a), ang(b), ang(c));
    let dm = (ma - sa).rem_euclid(TAU);
    let de = (ea - sa).rem_euclid(TAU);
    let (start, end) = if dm <= de {
        (sa, sa + de)
    } else {
        (ea, ea + (TAU - de))
    };
    Some(PcbShape::Arc {
        c: rp(center),
        r: r4(r),
        start,
        end,
    })
}

fn arc_or_segment(a: Pt, b: Pt, c: Pt) -> PcbShape {
    arc3(a, b, c).unwrap_or(PcbShape::Segment { a, b: c })
}

/// Points along an arc shape (for bounding boxes and polygon outlines).
fn arc_points(c: Pt, r: f64, start: f64, end: f64, out: &mut Vec<Pt>) {
    let steps = (((end - start).abs() / TAU) * 64.0).ceil().max(2.0) as usize;
    for i in 0..=steps {
        let a = start + (end - start) * i as f64 / steps as f64;
        out.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
    }
}

fn shape_points(shape: &PcbShape) -> Vec<Pt> {
    match shape {
        PcbShape::Segment { a, b } => vec![*a, *b],
        PcbShape::Arc { c, r, start, end } => {
            let mut out = Vec::new();
            arc_points(*c, *r, *start, *end, &mut out);
            out
        }
        PcbShape::Circle { c, r } => vec![[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]],
        PcbShape::Polygon { pts } | PcbShape::Polyline { pts } => pts.clone(),
    }
}

/// Polygon `(pts ...)` with `(xy)` and KiCad 7+ `(arc (start)(mid)(end))`.
fn pts_of(node: &SExp, xf: &Xform) -> Vec<Pt> {
    let Some(pts) = node.get("pts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for child in pts.children.iter().filter(|c| c.is_list()) {
        match child.tag() {
            Some("xy") => out.push(xf.apply(xy_of(child))),
            Some("arc") => {
                let (Some(a), Some(b), Some(c)) = (
                    child_xy(child, "start"),
                    child_xy(child, "mid"),
                    child_xy(child, "end"),
                ) else {
                    continue;
                };
                match arc3(xf.apply(a), xf.apply(b), xf.apply(c)) {
                    Some(PcbShape::Arc { c, r, start, end }) => {
                        let mut pts = Vec::new();
                        arc_points(c, r, start, end, &mut pts);
                        // Keep the file's traversal direction.
                        let first = xf.apply(a);
                        let near = |p: &Pt| (p[0] - first[0]).hypot(p[1] - first[1]);
                        if pts.first().map(near) > pts.last().map(near) {
                            pts.reverse();
                        }
                        out.extend(pts.into_iter().map(rp));
                    }
                    _ => out.extend([xf.apply(a), xf.apply(c)]),
                }
            }
            _ => {}
        }
    }
    out
}

fn bezier(p: [Pt; 4]) -> Vec<Pt> {
    (0..=24)
        .map(|i| {
            let t = i as f64 / 24.0;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            rp([
                (0..4).map(|k| w[k] * p[k][0]).sum(),
                (0..4).map(|k| w[k] * p[k][1]).sum(),
            ])
        })
        .collect()
}

fn stroke_width(node: &SExp) -> f64 {
    node.get("stroke")
        .and_then(|s| s.child_f64("width"))
        .or_else(|| node.child_f64("width"))
        .unwrap_or(0.0)
}

fn is_filled(node: &SExp) -> bool {
    node.get("fill")
        .and_then(|f| atom_text(f, 0))
        .is_some_and(|t| fill_token_is_filled(&t))
}

/// `gr_*` / `fp_*` drawing (suffix after the prefix) as a shape.
fn graphic_shape(node: &SExp, kind: &str, xf: &Xform) -> Option<PcbShape> {
    Some(match kind {
        "line" => PcbShape::Segment {
            a: xf.apply(child_xy(node, "start")?),
            b: xf.apply(child_xy(node, "end")?),
        },
        "rect" => {
            let a = child_xy(node, "start")?;
            let b = child_xy(node, "end")?;
            PcbShape::Polygon {
                pts: [a, [b[0], a[1]], b, [a[0], b[1]]]
                    .into_iter()
                    .map(|p| xf.apply(p))
                    .collect(),
            }
        }
        "circle" => {
            let c = child_xy(node, "center")?;
            let e = child_xy(node, "end")?;
            PcbShape::Circle {
                c: xf.apply(c),
                r: r4((e[0] - c[0]).hypot(e[1] - c[1])),
            }
        }
        "arc" => {
            let (a, b, c) = arc_points_from_sexp(node);
            arc_or_segment(
                xf.apply([a.0, a.1]),
                xf.apply([b.0, b.1]),
                xf.apply([c.0, c.1]),
            )
        }
        "poly" => PcbShape::Polygon {
            pts: pts_of(node, xf),
        },
        "curve" => {
            let pts = pts_of(node, xf);
            if pts.len() != 4 {
                return None;
            }
            PcbShape::Polyline {
                pts: bezier([pts[0], pts[1], pts[2], pts[3]]),
            }
        }
        _ => return None,
    })
}

// ------------------------------------------------------------------ builder

struct Builder<'a> {
    pcb: &'a Pcb,
    origin: Pt,
    board: PcbBoard,
    layer_index: HashMap<String, u16>,
    /// Copper layer names, front to back.
    copper: Vec<String>,
    net_index: HashMap<String, u32>,
}

fn copper_rank(name: &str) -> Option<usize> {
    match name {
        "F.Cu" => Some(0),
        "B.Cu" => Some(1000),
        _ => name
            .strip_prefix("In")
            .and_then(|n| n.strip_suffix(".Cu"))
            .and_then(|n| n.parse().ok()),
    }
}

impl<'a> Builder<'a> {
    fn new(pcb: &'a Pcb) -> Self {
        let o = pcb.board_origin();
        let mut b = Builder {
            pcb,
            origin: [o.0, o.1],
            board: PcbBoard {
                title: pcb.title().to_string(),
                ..PcbBoard::default()
            },
            layer_index: HashMap::new(),
            copper: Vec::new(),
            net_index: HashMap::new(),
        };
        // Aliases live in the raw `(layers (0 "F.Cu" signal "Top") ...)`.
        let mut aliases = HashMap::new();
        if let Some(table) = pcb.sexp().get("layers") {
            for entry in table.children.iter().filter(|c| c.is_list()) {
                if let (Some(name), Some(alias)) = (atom_text(entry, 0), atom_text(entry, 2)) {
                    aliases.insert(name, alias);
                }
            }
        }
        for layer in pcb.layers() {
            let kind = if layer.name.ends_with(".Cu") {
                "copper".to_string()
            } else {
                layer.layer_type.clone()
            };
            b.add_layer(&layer.name, &kind, aliases.get(&layer.name).cloned(), false);
        }
        if b.copper.is_empty() {
            b.add_layer("F.Cu", "copper", None, false);
            b.add_layer("B.Cu", "copper", None, false);
        }
        b.board.nets.push(String::new());
        b.net_index.insert(String::new(), 0);
        for net in pcb.nets() {
            b.net(&net.name);
        }
        b
    }

    fn add_layer(&mut self, name: &str, kind: &str, alias: Option<String>, disabled: bool) -> u16 {
        if let Some(&i) = self.layer_index.get(name) {
            return i;
        }
        let i = self.board.layers.len() as u16;
        self.board.layers.push(PcbLayer {
            name: name.to_string(),
            kind: kind.to_string(),
            alias: alias.unwrap_or_default(),
            disabled,
        });
        self.layer_index.insert(name.to_string(), i);
        if kind == "copper" {
            self.copper.push(name.to_string());
            self.copper.sort_by_key(|n| copper_rank(n).unwrap_or(500));
        }
        i
    }

    fn layer(&mut self, name: &str) -> u16 {
        let kind = if name.ends_with(".Cu") {
            "copper"
        } else {
            "user"
        };
        self.add_layer(name, kind, None, true)
    }

    /// Expand pad/zone layer wildcards (`*.Cu`, `F&B.Cu`, `*.Mask`, ...).
    fn layers(&mut self, names: &[String]) -> Vec<u16> {
        let mut out: Vec<u16> = Vec::new();
        for name in names {
            let expanded: Vec<String> = if name == "*.Cu" {
                self.copper.clone()
            } else if name == "F&B.Cu" {
                vec!["F.Cu".into(), "B.Cu".into()]
            } else if let Some(suffix) = name.strip_prefix("*.") {
                vec![format!("F.{suffix}"), format!("B.{suffix}")]
            } else if let Some(suffix) = name.strip_prefix("F&B.") {
                vec![format!("F.{suffix}"), format!("B.{suffix}")]
            } else {
                vec![name.clone()]
            };
            for n in expanded {
                let i = self.layer(&n);
                if !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out
    }

    fn net(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.net_index.get(name) {
            return i;
        }
        let i = self.board.nets.len() as u32;
        self.board.nets.push(name.to_string());
        self.net_index.insert(name.to_string(), i);
        i
    }

    fn net_of(&mut self, number: i64, name: &str) -> u32 {
        if !name.is_empty() {
            return self.net(name);
        }
        match self.pcb.get_net(number) {
            Some(n) if !n.name.is_empty() => {
                let name = n.name.clone();
                self.net(&name)
            }
            _ => 0,
        }
    }

    fn shift(&self, p: (f64, f64)) -> Pt {
        rp([p.0 + self.origin[0], p.1 + self.origin[1]])
    }

    fn copper(&mut self) {
        let pcb = self.pcb;
        for s in pcb.segments() {
            let layer = self.layer(&s.layer);
            let net = self.net_of(s.net_number, &s.net_name);
            self.board.tracks.push(PcbTrack {
                layer,
                net,
                width: r4(s.width),
                shape: PcbShape::Segment {
                    a: self.shift(s.start),
                    b: self.shift(s.end),
                },
            });
        }
        for a in pcb.arcs() {
            let layer = self.layer(&a.layer);
            let net = self.net_of(a.net_number, &a.net_name);
            let shape = arc_or_segment(self.shift(a.start), self.shift(a.mid), self.shift(a.end));
            self.board.tracks.push(PcbTrack {
                layer,
                net,
                width: r4(a.width),
                shape,
            });
        }
        for v in pcb.vias() {
            let net = self.net_of(v.net_number, &v.net_name);
            let ranks: Vec<usize> = v.layers.iter().filter_map(|l| copper_rank(l)).collect();
            let (lo, hi) = (
                ranks.iter().copied().min().unwrap_or(0),
                ranks.iter().copied().max().unwrap_or(1000),
            );
            let span: Vec<String> = self
                .copper
                .iter()
                .filter(|n| copper_rank(n).is_some_and(|r| r >= lo && r <= hi))
                .cloned()
                .collect();
            let layers = span.iter().map(|n| self.layer(n)).collect();
            let kind = match v.via_type.as_deref() {
                Some("micro") => "micro",
                Some("blind") if lo == 0 || hi == 1000 => "blind",
                Some("blind") => "buried",
                _ => "through",
            };
            self.board.vias.push(PcbVia {
                pos: self.shift(v.position),
                size: r4(v.size),
                drill: r4(v.drill),
                layers,
                net,
                kind: kind.to_string(),
            });
        }
        for z in pcb.zones() {
            let net = self.net_of(z.net_number, &z.net_name);
            let names = if z.layers.is_empty() {
                vec![z.layer.clone()]
            } else {
                z.layers.clone()
            };
            let layers = self.layers(&names);
            let stroke = if z.is_stroked_fill() {
                r4(z.min_thickness)
            } else {
                0.0
            };
            let mut fills = Vec::new();
            for (i, poly) in z.filled_polygons.iter().enumerate() {
                let layer = self.layer(z.filled_polygon_layer(i));
                fills.push(PcbZoneFill {
                    layer,
                    pts: poly.iter().map(|p| self.shift(*p)).collect(),
                    stroke,
                });
            }
            self.board.zones.push(PcbZone {
                net,
                name: z.name.clone(),
                layers,
                outline: z.polygon.iter().map(|p| self.shift(*p)).collect(),
                fills,
                keepout: z.is_rule_area(),
                priority: z.priority,
            });
        }
    }

    fn graphic(&mut self, node: &SExp, kind: &str, xf: &Xform) -> Option<PcbGraphic> {
        let layer = node.get("layer").and_then(|l| atom_text(l, 0))?;
        let shape = graphic_shape(node, kind, xf)?;
        let filled = is_filled(node)
            || (kind == "poly" && node.get("fill").is_none() && stroke_width(node) == 0.0);
        Some(PcbGraphic {
            layer: self.layer(&layer),
            shape,
            width: r4(stroke_width(node)),
            filled,
        })
    }

    /// Board graphics and text (`gr_*`).
    fn board_items(&mut self) {
        let root = self.pcb.sexp();
        for child in root.children.iter().filter(|c| c.is_list()) {
            let Some(tag) = child.tag() else { continue };
            if tag == "gr_text" {
                let text = atom_text(child, 0).unwrap_or_default();
                if let Some(t) = self.text(child, &text, "board", None) {
                    self.board.texts.push(t);
                }
            } else if let Some(kind) = tag.strip_prefix("gr_") {
                if let Some(g) = self.graphic(child, kind, &Xform::IDENTITY) {
                    self.board.graphics.push(g);
                }
            }
        }
    }

    /// Read a text node (`gr_text`, `fp_text`, footprint `property`) with
    /// its effects; `parent` is the footprint transform for footprint text.
    fn text(
        &mut self,
        node: &SExp,
        content: &str,
        kind: &str,
        parent: Option<&Xform>,
    ) -> Option<PcbText> {
        let layer = node.get("layer").and_then(|l| atom_text(l, 0))?;
        let effects = node.get("effects");
        let hidden = has_atom(node, "hide")
            || node
                .get("hide")
                .is_some_and(|h| h.string_at(0) != Some("no"))
            || effects.is_some_and(|e| yes(e, "hide"));
        if content.trim().is_empty() {
            return None;
        }
        let at = node.get("at")?;
        let local = xy_of(at);
        let mut angle = fnum(at, 2).unwrap_or(0.0);
        let pos = match parent {
            Some(xf) => xf.apply(local),
            None => rp(local),
        };
        if parent.is_some() && !has_atom(at, "unlocked") {
            // KiCad keeps footprint text readable: angle in (-90, 90].
            angle = angle.rem_euclid(360.0);
            if angle > 90.0 && angle <= 270.0 {
                angle -= 180.0;
            } else if angle > 270.0 {
                angle -= 360.0;
            }
        }
        let font = effects.and_then(|e| e.get("font"));
        let (mut h, mut w) = (1.0, 1.0);
        if let Some(size) = font.and_then(|f| f.get("size")) {
            h = fnum(size, 0).unwrap_or(1.0);
            w = fnum(size, 1).unwrap_or(h);
        }
        let bold = font.is_some_and(|f| yes(f, "bold"));
        let italic = font.is_some_and(|f| yes(f, "italic"));
        let mut thickness = font.and_then(|f| f.child_f64("thickness")).unwrap_or(0.0);
        if thickness <= 0.0 {
            thickness = if bold { w / 5.0 } else { w / 8.0 };
        }
        thickness = thickness.min(w / 4.0);
        let line_spacing = font
            .and_then(|f| f.child_f64("line_spacing"))
            .unwrap_or(1.0);
        let justify = effects.and_then(|e| e.get("justify"));
        let j = |t: &str| justify.is_some_and(|n| has_atom(n, t));
        let pick = |a: &'static str, b: &'static str| -> &'static str {
            if j(a) {
                a
            } else if j(b) {
                b
            } else {
                "center"
            }
        };
        Some(PcbText {
            layer: self.layer(&layer),
            text: content.to_string(),
            kind: kind.to_string(),
            pos,
            angle,
            size: rp([w, h]),
            thickness: r4(thickness),
            h_align: pick("left", "right").to_string(),
            v_align: pick("top", "bottom").to_string(),
            mirrored: j("mirror"),
            italic,
            bold,
            line_spacing,
            visible: !hidden,
        })
    }

    fn footprints(&mut self) {
        let pcb = self.pcb;
        let nodes = pcb
            .sexp()
            .children
            .iter()
            .filter(|c| is_footprint_tag(c.tag()));
        for (typed, node) in pcb.footprints().iter().zip(nodes) {
            let (x, y, angle) = node.at().unwrap_or((0.0, 0.0, 0.0));
            let xf = Xform {
                origin: [x, y],
                angle,
            };
            let mut fp = PcbFootprint {
                reference: typed.reference.clone(),
                value: typed.value.clone(),
                footprint: typed.name.clone(),
                layer: self.layer(&typed.layer),
                pos: rp([x, y]),
                angle,
                description: typed.description.clone(),
                attr: typed.attr.clone(),
                dnp: typed.dnp,
                properties: typed.properties.clone(),
                ..PcbFootprint::default()
            };
            let substitute = |text: &str| {
                text.replace("${REFERENCE}", &typed.reference)
                    .replace("${VALUE}", &typed.value)
                    .replace("%R", &typed.reference)
                    .replace("%V", &typed.value)
            };
            let mut local_bounds: Vec<Pt> = Vec::new();
            for child in node.children.iter().filter(|c| c.is_list()) {
                let Some(tag) = child.tag() else { continue };
                match tag {
                    "pad" => {
                        if let Some(pad) = self.pad(child, &xf, angle, &mut local_bounds) {
                            fp.pads.push(pad);
                        }
                    }
                    "fp_text" => {
                        let kind = atom_text(child, 0).unwrap_or_else(|| "user".into());
                        let text = substitute(&atom_text(child, 1).unwrap_or_default());
                        if let Some(t) = self.text(child, &text, &kind, Some(&xf)) {
                            fp.texts.push(t);
                        }
                    }
                    "property" => {
                        let name = atom_text(child, 0).unwrap_or_default();
                        let kind = match name.as_str() {
                            "Reference" => "reference",
                            "Value" => "value",
                            _ => "user",
                        };
                        let text = substitute(&atom_text(child, 1).unwrap_or_default());
                        if let Some(t) = self.text(child, &text, kind, Some(&xf)) {
                            fp.texts.push(t);
                        }
                    }
                    _ => {
                        let Some(kind) = tag.strip_prefix("fp_") else {
                            continue;
                        };
                        if let Some(g) = self.graphic(child, kind, &xf) {
                            let layer = &self.board.layers[g.layer as usize].name;
                            if !layer.ends_with(".CrtYd") {
                                let local = Xform::IDENTITY;
                                if let Some(shape) = graphic_shape(child, kind, &local) {
                                    local_bounds.extend(shape_points(&shape));
                                }
                            }
                            fp.graphics.push(g);
                        }
                    }
                }
            }
            fp.bbox = oriented_bbox(&local_bounds, &xf);
            self.board.footprints.push(fp);
        }
    }

    fn pad(
        &mut self,
        node: &SExp,
        xf: &Xform,
        fp_angle: f64,
        bounds: &mut Vec<Pt>,
    ) -> Option<PcbPad> {
        let at = node.get("at")?;
        let local = xy_of(at);
        let angle = fnum(at, 2).unwrap_or(0.0);
        let size = node
            .get("size")
            .map(|s| {
                let w = fnum(s, 0).unwrap_or(0.0);
                [w, fnum(s, 1).unwrap_or(w)]
            })
            .unwrap_or([0.0, 0.0]);
        let layer_names: Vec<String> = node
            .get("layers")
            .map(|l| {
                (0..l.children.len())
                    .filter_map(|i| atom_text(l, i))
                    .collect()
            })
            .unwrap_or_default();
        let net_name = node
            .get("net")
            .and_then(|n| {
                (0..n.children.len())
                    .rev()
                    .find_map(|i| n.value_at(i).and_then(Value::as_str).map(str::to_owned))
            })
            .unwrap_or_default();
        let net_number = node.get("net").and_then(|n| n.int_at(0)).unwrap_or(0);
        let mut pad = PcbPad {
            number: atom_text(node, 0).unwrap_or_default(),
            kind: atom_text(node, 1).unwrap_or_default(),
            shape: atom_text(node, 2).unwrap_or_default(),
            pos: xf.apply(local),
            angle,
            size: rp(size),
            layers: self.layers(&layer_names),
            net: self.net_of(net_number, &net_name),
            rratio: node.child_f64("roundrect_rratio").unwrap_or(0.0),
            chamfer_ratio: node.child_f64("chamfer_ratio").unwrap_or(0.0),
            pin_function: node
                .get("pinfunction")
                .and_then(|p| atom_text(p, 0))
                .unwrap_or_default(),
            ..PcbPad::default()
        };
        if pad.shape == "roundrect" && pad.rratio == 0.0 && node.get("roundrect_rratio").is_none() {
            pad.rratio = 0.25;
        }
        if let Some(ch) = node.get("chamfer") {
            for (bit, name) in [
                (1, "top_left"),
                (2, "top_right"),
                (4, "bottom_left"),
                (8, "bottom_right"),
            ] {
                if has_atom(ch, name) {
                    pad.chamfer |= bit;
                }
            }
        }
        if let Some(d) = child_xy(node, "rect_delta") {
            pad.delta = rp(d);
        }
        if let Some(drill) = node.get("drill") {
            let oval = has_atom(drill, "oval");
            let nums: Vec<f64> = drill
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_atom())
                .filter_map(|(i, _)| drill.float_at(i))
                .collect();
            if let Some(&d) = nums.first() {
                let h = if oval {
                    nums.get(1).copied().unwrap_or(d)
                } else {
                    d
                };
                if d > 0.0 {
                    pad.drill = Some(PcbDrill {
                        oval,
                        size: rp([d, h]),
                    });
                }
            }
            if let Some(o) = child_xy(drill, "offset") {
                pad.offset = rp(o);
            }
        }
        if pad.shape == "custom" {
            pad.custom_anchor = node
                .get("options")
                .and_then(|o| o.get("anchor"))
                .and_then(|a| atom_text(a, 0))
                .unwrap_or_else(|| "circle".into());
            if let Some(prims) = node.get("primitives") {
                for prim in prims.children.iter().filter(|c| c.is_list()) {
                    let Some(kind) = prim.tag().and_then(|t| t.strip_prefix("gr_")) else {
                        continue;
                    };
                    let Some(shape) = graphic_shape(prim, kind, &Xform::IDENTITY) else {
                        continue;
                    };
                    let width = stroke_width(prim);
                    let filled = is_filled(prim) || (kind == "poly" && prim.get("fill").is_none());
                    pad.custom.push(PcbGraphic {
                        layer: 0,
                        shape,
                        width: r4(width),
                        filled,
                    });
                }
            }
        }
        // Footprint-frame extent of the pad for the footprint bbox.
        let rel = angle - fp_angle;
        let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
        let mut corners = vec![[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]];
        for prim in &pad.custom {
            corners.extend(shape_points(&prim.shape));
        }
        for c in corners {
            let p = rotate([c[0] + pad.offset[0], c[1] + pad.offset[1]], rel);
            bounds.push([p[0] + local[0], p[1] + local[1]]);
        }
        Some(pad)
    }

    fn bbox(&self) -> [f64; 4] {
        let mut b = EMPTY_BOUNDS;
        let edge = self.layer_index.get("Edge.Cuts").copied();
        let graphics = self
            .board
            .graphics
            .iter()
            .chain(self.board.footprints.iter().flat_map(|f| f.graphics.iter()));
        for g in graphics.filter(|g| Some(g.layer) == edge) {
            grow(&mut b, &shape_points(&g.shape));
        }
        if b[0] > b[2] {
            for fp in &self.board.footprints {
                grow(&mut b, &fp.bbox);
            }
            for t in &self.board.tracks {
                grow(&mut b, &shape_points(&t.shape));
            }
            for g in &self.board.graphics {
                grow(&mut b, &shape_points(&g.shape));
            }
        }
        if b[0] > b[2] {
            return [0.0, 0.0, 100.0, 100.0];
        }
        b.map(r4)
    }
}

const EMPTY_BOUNDS: [f64; 4] = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];

fn grow(b: &mut [f64; 4], pts: &[Pt]) {
    for p in pts {
        *b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
}

fn oriented_bbox(local: &[Pt], xf: &Xform) -> Vec<Pt> {
    let mut b = EMPTY_BOUNDS;
    grow(&mut b, local);
    if b[0] > b[2] {
        b = [-0.5, -0.5, 0.5, 0.5];
    }
    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
        .into_iter()
        .map(|p| xf.apply(p))
        .collect()
}

/// Parse `.kicad_pcb` text into the view model.
pub(crate) fn build_board(text: &str) -> Result<PcbBoard> {
    let pcb = Pcb::parse_str(text)?;
    let mut b = Builder::new(&pcb);
    b.copper();
    b.footprints();
    b.board_items();
    b.board.bbox = b.bbox();
    b.board.warnings = pcb.parse_warnings.clone();
    if !pcb.outline_error.is_empty() {
        b.board.warnings.push(pcb.outline_error.clone());
    }
    Ok(b.board)
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
    (pad "2" thru_hole oval (at 2.54 0) (size 1.7 2.2) (drill oval 0.8 1.2 (offset 0 0.1)) (layers "*.Cu" "*.Mask") (net 2 "VCC"))
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

    #[test]
    fn builds_board_in_sheet_coordinates() {
        let board = build_board(BOARD).unwrap();
        assert_eq!(board.bbox, [100.0, 100.0, 140.0, 120.0]);
        let names: Vec<_> = board.layers.iter().map(|l| l.name.as_str()).collect();
        assert!(names.contains(&"F.Cu") && names.contains(&"Edge.Cuts"));
        assert_eq!(board.layers[2].alias, "B.Silkscreen");
        assert_eq!(board.nets[1], "GND");

        let r1 = &board.footprints[0];
        assert_eq!(r1.reference, "R1");
        assert_eq!(r1.value, "10k");
        // Pad at local (-0.9, 0), footprint rotated 90 CCW: lands at (110, 110.9).
        assert_eq!(r1.pads[0].pos, [110.0, 110.9]);
        assert_eq!(r1.pads[0].rratio, 0.25);
        assert_eq!(board.nets[r1.pads[1].net as usize], "VCC");
        // The hidden value is sent but flagged; the reference keeps its effects.
        assert_eq!(r1.texts.len(), 2);
        assert!(r1.texts[0].visible && !r1.texts[1].visible);
        assert_eq!(r1.texts[0].size, [1.0, 1.0]);
        assert_eq!(r1.texts[0].h_align, "center");
        assert_eq!(board.texts[0].h_align, "left");
        assert_eq!(r1.bbox.len(), 4);

        let j1 = &board.footprints[1];
        let fcu = board.layers.iter().position(|l| l.name == "F.Cu").unwrap() as u16;
        let bcu = board.layers.iter().position(|l| l.name == "B.Cu").unwrap() as u16;
        assert!(j1.pads[0].layers.contains(&fcu) && j1.pads[0].layers.contains(&bcu));
        let drill = j1.pads[1].drill.as_ref().unwrap();
        assert!(drill.oval);
        assert_eq!(drill.size, [0.8, 1.2]);
        assert_eq!(j1.pads[1].offset, [0.0, 0.1]);
        assert_eq!(j1.texts[0].kind, "reference");

        assert_eq!(board.tracks.len(), 2);
        assert!(matches!(
            board.tracks[0].shape,
            PcbShape::Segment {
                a: [110.0, 109.1],
                ..
            }
        ));
        match &board.tracks[1].shape {
            PcbShape::Arc { c, r, start, end } => {
                assert!((c[0] - 122.0).abs() < 1e-6 && (c[1] - 105.0).abs() < 1e-6);
                assert!((r - 2.0).abs() < 1e-6);
                // Through the top (y = 103, angle -pi/2).
                let mid = -std::f64::consts::FRAC_PI_2;
                let swept = (mid - start).rem_euclid(TAU) <= (end - start);
                assert!(swept, "{start} {end}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(board.vias[0].layers.len(), 2);
        assert_eq!(board.vias[0].pos, [125.0, 105.0]);
        assert_eq!(board.zones[0].fills[0].pts[0], [101.0, 101.0]);
        assert_eq!(board.zones[0].fills[0].layer, bcu);
        assert_eq!(board.texts[0].text, "HELLO");
    }

    #[test]
    fn keep_upright_flips_footprint_text() {
        let text = BOARD.replace(r#"(at 0 -1.5 90)"#, r#"(at 0 -1.5 180)"#);
        let board = build_board(&text).unwrap();
        let t = &board.footprints[0].texts[0];
        // 180 degrees reads upside down, so KiCad draws it at 0.
        assert_eq!(t.angle, 0.0);
        let unlocked = text.replace(r#"(at 0 -1.5 180)"#, r#"(at 0 -1.5 180 unlocked)"#);
        let board = build_board(&unlocked).unwrap();
        assert_eq!(board.footprints[0].texts[0].angle, 180.0);
    }
}
