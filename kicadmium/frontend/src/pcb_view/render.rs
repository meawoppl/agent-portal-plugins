//! Canvas2D painter for the PCB view. Geometry is batched once per board
//! (and per highlighted net) into `Path2d`s per paint pass, so a frame is a
//! handful of fill/stroke calls under the view transform. Canvas usage and
//! the stroke/fill conventions are derived from pastebom's
//! `crates/viewer/src/render.rs` (see below).
//!
//! THIRD-PARTY PROVENANCE: parts of this module are derived from pastebom's
//! viewer (github.com/meawoppl/pastebom.com, `crates/viewer`), same author.
//! Its author explicitly authorized the listed derived parts under
//! kicadmium's MIT license. Items and provenance are recorded in
//! `docs/third-party.md`.

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;

use shared::pcb::{PcbBoard, PcbShape, PcbText, Pt};
use web_sys::{CanvasRenderingContext2d, Path2d};

use super::geom::{drill_piece, pad_pieces, signed_area, Hit, Piece, View};
use super::text::draw_texts;
use super::theme::{paint_order, Pass, Rgba, Theme};

/// Fills plus width-grouped strokes, in board coordinates.
#[derive(Default)]
pub struct Group {
    fill: Option<Path2d>,
    strokes: Vec<(f64, Path2d)>,
}

#[derive(Default)]
struct GroupBuilder {
    fill: Option<Path2d>,
    strokes: HashMap<u64, (f64, Path2d)>,
}

fn new_path() -> Path2d {
    Path2d::new().expect("Path2d")
}

impl GroupBuilder {
    fn fill_path(&mut self) -> &Path2d {
        self.fill.get_or_insert_with(new_path)
    }

    fn stroke_path(&mut self, width: f64) -> &Path2d {
        &self
            .strokes
            .entry(width.to_bits())
            .or_insert_with(|| (width, new_path()))
            .1
    }

    /// Filled outlines all wind the same way so one nonzero fill unions them.
    fn polygon(&mut self, pts: &[Pt]) {
        if pts.len() < 3 {
            return;
        }
        let path = self.fill_path();
        let mut it: Box<dyn Iterator<Item = &Pt>> = if signed_area(pts) >= 0.0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        let first = it.next().unwrap();
        path.move_to(first[0], first[1]);
        for p in it {
            path.line_to(p[0], p[1]);
        }
        path.close_path();
    }

    fn circle(&mut self, c: Pt, r: f64) {
        if r <= 0.0 {
            return;
        }
        let path = self.fill_path();
        path.move_to(c[0] + r, c[1]);
        let _ = path.arc(c[0], c[1], r, 0.0, TAU);
        path.close_path();
    }

    fn polyline(&mut self, pts: &[Pt], width: f64) {
        if pts.is_empty() {
            return;
        }
        let path = self.stroke_path(width);
        path.move_to(pts[0][0], pts[0][1]);
        if pts.len() == 1 {
            path.line_to(pts[0][0], pts[0][1]);
        }
        for p in &pts[1..] {
            path.line_to(p[0], p[1]);
        }
    }

    fn piece(&mut self, piece: &Piece) {
        match piece {
            Piece::Circle(c, r) => self.circle(*c, *r),
            Piece::Polygon(pts) => self.polygon(pts),
            Piece::Stroke(pts, w) => self.polyline(pts, *w),
        }
    }

    /// Centre-line stroke of a track or drawing shape.
    fn shape(&mut self, shape: &PcbShape, width: f64, filled: bool) {
        match shape {
            PcbShape::Segment { a, b } => self.polyline(&[*a, *b], width),
            PcbShape::Arc { c, r, start, end } => {
                let path = self.stroke_path(width);
                path.move_to(c[0] + r * start.cos(), c[1] + r * start.sin());
                let _ = path.arc(c[0], c[1], *r, *start, *end);
            }
            PcbShape::Circle { c, r } => {
                if filled {
                    self.circle(*c, r + width / 2.0);
                } else {
                    let path = self.stroke_path(width);
                    path.move_to(c[0] + r, c[1]);
                    let _ = path.arc(c[0], c[1], *r, 0.0, TAU);
                }
            }
            PcbShape::Polygon { pts } => {
                if filled {
                    self.polygon(pts);
                }
                if width > 0.0 || !filled {
                    let mut ring = pts.clone();
                    if let Some(first) = pts.first() {
                        ring.push(*first);
                    }
                    self.polyline(&ring, width);
                }
            }
            PcbShape::Polyline { pts } => self.polyline(pts, width),
        }
    }

    fn finish(self) -> Group {
        let mut strokes: Vec<(f64, Path2d)> = self.strokes.into_values().collect();
        strokes.sort_by(|a, b| a.0.total_cmp(&b.0));
        Group {
            fill: self.fill,
            strokes,
        }
    }
}

/// Normal and highlighted-net groups of one pass.
#[derive(Default)]
struct PassPaths {
    normal: Group,
    net: Group,
    /// Text drawn with `fillText` in this pass (see `text.rs`).
    texts: Vec<PcbText>,
}

/// Footprint text classes shown (bit set).
pub const TEXT_REFERENCES: u8 = 1;
pub const TEXT_VALUES: u8 = 2;
/// Footprint user text off the silkscreen (fab-layer `${REFERENCE}` copies,
/// drawing notes). Silkscreen user text always follows its layer.
pub const TEXT_USER: u8 = 4;

fn text_shown(mask: u8, text: &PcbText, board: &PcbBoard) -> bool {
    let class = match text.kind.as_str() {
        "reference" => TEXT_REFERENCES,
        "value" => TEXT_VALUES,
        _ if board.layers[text.layer as usize].name.ends_with(".SilkS") => return true,
        _ => TEXT_USER,
    };
    mask & class != 0
}

/// Board paths cached for a highlight / text-visibility state.
pub struct Scene {
    passes: HashMap<Pass, PassPaths>,
    highlight: u32,
    text: u8,
}

#[derive(Default)]
struct SceneBuilder {
    passes: HashMap<Pass, (GroupBuilder, GroupBuilder, Vec<PcbText>)>,
}

impl SceneBuilder {
    fn group(&mut self, pass: Pass, on_net: bool) -> &mut GroupBuilder {
        let entry = self.passes.entry(pass).or_default();
        if on_net {
            &mut entry.1
        } else {
            &mut entry.0
        }
    }

    fn text(&mut self, pass: Pass, text: &PcbText) {
        if text.visible {
            self.passes.entry(pass).or_default().2.push(text.clone());
        }
    }
}

impl Scene {
    /// Batch `board` for net `highlight` (0 = none) and the `text` classes.
    pub fn build(board: &PcbBoard, highlight: u32, text: u8) -> Scene {
        let name = |l: u16| board.layers[l as usize].name.clone();
        let is_copper = |l: u16| board.layers[l as usize].name.ends_with(".Cu");
        let lit = |net: u32| highlight != 0 && net == highlight;
        let mut b = SceneBuilder::default();
        for g in &board.graphics {
            b.group(Pass::Layer(name(g.layer)), false)
                .shape(&g.shape, g.width, g.filled);
        }
        for t in &board.texts {
            b.text(Pass::Layer(name(t.layer)), t);
        }
        for t in &board.tracks {
            b.group(Pass::Layer(name(t.layer)), lit(t.net))
                .shape(&t.shape, t.width, false);
        }
        for fp in &board.footprints {
            for g in &fp.graphics {
                b.group(Pass::Layer(name(g.layer)), false)
                    .shape(&g.shape, g.width, g.filled);
            }
            for t in fp.texts.iter().filter(|t| text_shown(text, t, board)) {
                b.text(Pass::Layer(name(t.layer)), t);
            }
            for pad in &fp.pads {
                let pieces = pad_pieces(pad);
                let on = lit(pad.net);
                // Copper only: like KiCanvas, mask/paste apertures are not
                // painted (they would tint every pad under translucent copper).
                for &l in pad.layers.iter().filter(|l| is_copper(**l)) {
                    let g = b.group(Pass::Pads(name(l)), on);
                    for piece in &pieces {
                        g.piece(piece);
                    }
                }
                match pad.kind.as_str() {
                    "thru_hole" => {
                        let g = b.group(Pass::PadWalls, on);
                        for piece in &pieces {
                            g.piece(piece);
                        }
                        if let Some(hole) = drill_piece(pad) {
                            b.group(Pass::PadHoles, false).piece(&hole);
                        }
                    }
                    "np_thru_hole" => {
                        if let Some(hole) = drill_piece(pad) {
                            b.group(Pass::NonPlatedHoles, false).piece(&hole);
                        }
                    }
                    _ => {}
                }
            }
        }
        for v in &board.vias {
            let on = lit(v.net);
            b.group(Pass::ViaWalls, on).circle(v.pos, v.size / 2.0);
            b.group(Pass::ViaHoles, false).circle(v.pos, v.drill / 2.0);
        }
        for z in board.zones.iter().filter(|z| !z.keepout) {
            let on = lit(z.net);
            for f in &z.fills {
                let g = b.group(Pass::Zones(name(f.layer)), on);
                g.polygon(&f.pts);
                if f.stroke > 0.0 {
                    let mut ring = f.pts.clone();
                    if let Some(first) = f.pts.first() {
                        ring.push(*first);
                    }
                    g.polyline(&ring, f.stroke);
                }
            }
        }
        Scene {
            passes: b
                .passes
                .into_iter()
                .map(|(k, (n, h, texts))| {
                    (
                        k,
                        PassPaths {
                            normal: n.finish(),
                            net: h.finish(),
                            texts,
                        },
                    )
                })
                .collect(),
            highlight,
            text,
        }
    }

    /// Whether this cache was built for `highlight` and `text`.
    pub fn matches(&self, highlight: u32, text: u8) -> bool {
        self.highlight == highlight && self.text == text
    }
}

/// Everything a frame depends on besides the cached scene.
pub struct Frame<'a> {
    pub board: &'a PcbBoard,
    pub theme: &'a Theme,
    pub view: View,
    /// Canvas size in CSS pixels.
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
    pub hidden: &'a HashSet<String>,
    pub pours: bool,
    pub selection: Option<Hit>,
}

impl Frame<'_> {
    fn layer_visible(&self, name: &str) -> bool {
        !self.hidden.contains(name)
    }

    fn any_copper_visible(&self) -> bool {
        self.board
            .layers
            .iter()
            .any(|l| l.kind == "copper" && self.layer_visible(&l.name))
    }

    /// Colour of a pass (alpha = blend opacity); `None` when hidden.
    fn pass_style(&self, pass: &Pass) -> Option<Rgba> {
        let t = self.theme;
        let holes = self.any_copper_visible();
        match pass {
            Pass::Layer(n) | Pass::Pads(n) => self.layer_visible(n).then(|| t.layer(n)),
            // KiCanvas zone layers: copper colour at the pours opacity.
            Pass::Zones(n) => (self.pours && self.layer_visible(n)).then(|| {
                let c = t.layer(n);
                c.with_alpha(c.3 * 0.6)
            }),
            Pass::ViaWalls => holes.then_some(t.via_wall),
            Pass::PadWalls => holes.then_some(t.pad_wall),
            Pass::PadHoles => holes.then_some(t.background.with_alpha(0.8)),
            Pass::ViaHoles => holes.then_some(t.via_hole),
            Pass::NonPlatedHoles => holes.then_some(t.npth),
        }
    }
}

fn paint_group(ctx: &CanvasRenderingContext2d, group: &Group, color: &str, min_width: f64) {
    if let Some(fill) = &group.fill {
        ctx.set_fill_style_str(color);
        ctx.fill_with_path_2d(fill);
    }
    if !group.strokes.is_empty() {
        ctx.set_stroke_style_str(color);
        for (w, path) in &group.strokes {
            ctx.set_line_width(w.max(min_width));
            ctx.stroke_with_path(path);
        }
    }
}

fn grid_step(scale: f64) -> f64 {
    // Decades only, like the KiCanvas grid (1 mm, then 10 mm when dense).
    let mut step = 0.01;
    while step * scale < 15.0 && step < 1000.0 {
        step *= 10.0;
    }
    step
}

fn draw_grid(ctx: &CanvasRenderingContext2d, f: &Frame) {
    let step = grid_step(f.view.scale);
    let a = f.view.to_board([0.0, 0.0]);
    let b = f.view.to_board([f.width, f.height]);
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    let path = new_path();
    let size = 2.0 * f.dpr;
    let mut x = (x0 / step).floor() * step;
    let mut count = 0;
    while x <= x1 && count < 40_000 {
        let mut y = (y0 / step).floor() * step;
        while y <= y1 && count < 40_000 {
            let s = f.view.to_screen([x, y]);
            path.rect(
                s[0] * f.dpr - size / 2.0,
                s[1] * f.dpr - size / 2.0,
                size,
                size,
            );
            y += step;
            count += 1;
        }
        x += step;
    }
    ctx.set_fill_style_str(&f.theme.grid.css());
    ctx.fill_with_path_2d(&path);
}

/// Paint one frame.
pub fn draw(ctx: &CanvasRenderingContext2d, scene: &Scene, f: &Frame) {
    let dpr = f.dpr;
    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    ctx.set_global_alpha(1.0);
    ctx.set_fill_style_str(&f.theme.background.css());
    ctx.fill_rect(0.0, 0.0, f.width * dpr, f.height * dpr);
    draw_grid(ctx, f);

    let v = f.view;
    let _ = ctx.set_transform(
        dpr * v.sx(),
        0.0,
        0.0,
        dpr * v.scale,
        dpr * v.tx,
        dpr * v.ty,
    );
    ctx.set_line_cap("round");
    ctx.set_line_join("round");
    let min_width = 1.0 / (v.scale * dpr);
    let names: Vec<String> = f.board.layers.iter().map(|l| l.name.clone()).collect();
    let dimming = scene.highlight != 0;
    for pass in paint_order(&names, v.flipped) {
        let Some(paths) = scene.passes.get(&pass) else {
            continue;
        };
        let Some(color) = f.pass_style(&pass) else {
            continue;
        };
        let opaque = color.with_alpha(1.0);
        if dimming {
            // Net highlight: everything else fades to grey (KiCanvas
            // `filter_net`), the net keeps its colour.
            ctx.set_global_alpha(color.3 * 0.5);
            let grey = opaque.mix(Rgba(120.0, 120.0, 120.0, 1.0), 0.75);
            paint_group(ctx, &paths.normal, &grey.css(), min_width);
            draw_texts(ctx, &paths.texts, &grey.css());
            ctx.set_global_alpha(color.3.max(0.85));
            paint_group(ctx, &paths.net, &opaque.css(), min_width);
        } else {
            ctx.set_global_alpha(color.3);
            paint_group(ctx, &paths.normal, &opaque.css(), min_width);
            draw_texts(ctx, &paths.texts, &opaque.css());
        }
    }
    ctx.set_global_alpha(1.0);
    if let Some(hit) = f.selection {
        draw_selection(ctx, f, hit, min_width);
    }
}

fn draw_selection(ctx: &CanvasRenderingContext2d, f: &Frame, hit: Hit, min_width: f64) {
    let sel = f.theme.selection;
    let mut g = GroupBuilder::default();
    let mut outline: Vec<Vec<Pt>> = Vec::new();
    let board = f.board;
    match hit {
        Hit::Footprint(i) => {
            let fp = &board.footprints[i];
            for pad in &fp.pads {
                for piece in pad_pieces(pad) {
                    g.piece(&piece);
                }
            }
            for gr in &fp.graphics {
                g.shape(&gr.shape, gr.width, gr.filled);
            }
            let mut ring = fp.bbox.clone();
            if let Some(first) = fp.bbox.first() {
                ring.push(*first);
            }
            outline.push(ring);
        }
        Hit::Pad(fi, pi) => {
            for piece in pad_pieces(&board.footprints[fi].pads[pi]) {
                g.piece(&piece);
            }
            let mut ring = board.footprints[fi].bbox.clone();
            if let Some(first) = ring.first().copied() {
                ring.push(first);
            }
            outline.push(ring);
        }
        Hit::Via(i) => {
            let v = &board.vias[i];
            g.circle(v.pos, v.size / 2.0);
        }
        Hit::Track(i) => {
            let t = &board.tracks[i];
            g.shape(&t.shape, t.width, false);
        }
        Hit::Zone(i) => {
            let mut ring = board.zones[i].outline.clone();
            if let Some(first) = ring.first().copied() {
                ring.push(first);
            }
            outline.push(ring);
        }
    }
    let group = g.finish();
    ctx.set_global_alpha(0.55);
    paint_group(ctx, &group, &sel.css(), min_width);
    ctx.set_global_alpha(1.0);
    if !outline.is_empty() {
        let path = new_path();
        for ring in &outline {
            if let Some(first) = ring.first() {
                path.move_to(first[0], first[1]);
                for p in &ring[1..] {
                    path.line_to(p[0], p[1]);
                }
            }
        }
        ctx.set_stroke_style_str(&sel.css());
        ctx.set_line_width(2.0 * min_width);
        ctx.stroke_with_path(&path);
    }
}

/// Pick tolerance in mm for a few screen pixels.
pub fn pick_slop(view: &View) -> f64 {
    4.0 / view.scale
}
