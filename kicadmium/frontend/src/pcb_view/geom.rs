//! Pure geometry for the PCB view: pad outlines in board coordinates, view
//! transform, and hit testing. Pad shapes, the view transform and the
//! hit-test helpers are derived from pastebom's `crates/viewer/src/render.rs`
//! (see below), generalised from its iBOM model to KiCad's full pad set
//! (roundrect, chamfer, trapezoid, custom).
//!
//! THIRD-PARTY PROVENANCE: parts of this module are derived from pastebom's
//! viewer (github.com/meawoppl/pastebom.com, `crates/viewer`), same author.
//! Its author explicitly authorized the listed derived parts under
//! kicadmium's MIT license. Items and provenance are recorded in
//! `docs/third-party.md`.

use std::f64::consts::{PI, TAU};

use shared::pcb::{PcbBoard, PcbGraphic, PcbPad, PcbShape, Pt};

/// Board-to-screen transform in CSS pixels: `screen = board * scale + t`,
/// with x negated when viewing from the back.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub scale: f64,
    pub tx: f64,
    pub ty: f64,
    pub flipped: bool,
}

impl Default for View {
    fn default() -> Self {
        View {
            scale: 10.0,
            tx: 0.0,
            ty: 0.0,
            flipped: false,
        }
    }
}

impl View {
    pub fn sx(&self) -> f64 {
        if self.flipped {
            -self.scale
        } else {
            self.scale
        }
    }

    pub fn to_screen(self, p: Pt) -> Pt {
        [p[0] * self.sx() + self.tx, p[1] * self.scale + self.ty]
    }

    pub fn to_board(self, s: Pt) -> Pt {
        [(s[0] - self.tx) / self.sx(), (s[1] - self.ty) / self.scale]
    }

    /// Fill fraction of [`View::fit`]: the board outline takes ~78% of the
    /// limiting dimension, matching how KiCanvas frames the same boards.
    pub const FIT_FILL: f64 = 0.78;

    /// Centre `bbox` in a `w`x`h` viewport at [`View::FIT_FILL`].
    pub fn fit(bbox: [f64; 4], w: f64, h: f64, flipped: bool) -> View {
        let bw = (bbox[2] - bbox[0]).max(1e-3);
        let bh = (bbox[3] - bbox[1]).max(1e-3);
        let scale = ((w / bw).min(h / bh) * Self::FIT_FILL).max(1e-3);
        let cx = (bbox[0] + bbox[2]) / 2.0;
        let cy = (bbox[1] + bbox[3]) / 2.0;
        let mut v = View {
            scale,
            tx: 0.0,
            ty: 0.0,
            flipped,
        };
        v.tx = w / 2.0 - cx * v.sx();
        v.ty = h / 2.0 - cy * scale;
        v
    }

    /// Zoom by `factor` keeping screen point `at` fixed.
    pub fn zoom_at(&mut self, at: Pt, factor: f64) {
        let b = self.to_board(at);
        self.scale = (self.scale * factor).clamp(0.05, 20000.0);
        self.tx = at[0] - b[0] * self.sx();
        self.ty = at[1] - b[1] * self.scale;
    }

    /// Mirror the view about the screen point `at`'s board x.
    pub fn flip_at(&mut self, at: Pt) {
        let b = self.to_board(at);
        self.flipped = !self.flipped;
        self.tx = at[0] - b[0] * self.sx();
    }
}

/// KiCad rotation: counter-clockwise on screen (y down).
pub fn rotate(p: Pt, deg: f64) -> Pt {
    if deg == 0.0 {
        return p;
    }
    let (s, c) = deg.to_radians().sin_cos();
    [p[0] * c + p[1] * s, -p[0] * s + p[1] * c]
}

/// Pad-frame to board transform (hole at `pos`, shape at `offset`).
pub fn pad_to_board(pad: &PcbPad, q: Pt) -> Pt {
    let r = rotate([q[0] + pad.offset[0], q[1] + pad.offset[1]], pad.angle);
    [pad.pos[0] + r[0], pad.pos[1] + r[1]]
}

/// Filled or stroked piece of a pad outline in board coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Circle(Pt, f64),
    Polygon(Vec<Pt>),
    /// Stroked centre line of `width` (custom-pad line/arc primitives).
    Stroke(Vec<Pt>, f64),
}

fn arc_pts(c: Pt, r: f64, a0: f64, a1: f64, steps: usize, out: &mut Vec<Pt>) {
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f64 / steps as f64;
        out.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
    }
}

/// Rounded/chamfered rectangle outline centred on the origin (pad frame,
/// increasing-angle order). `chamfer` bits: 1 TL, 2 TR, 4 BL, 8 BR.
pub fn rect_outline(size: Pt, radius: f64, chamfer: u8, chamfer_len: f64) -> Vec<Pt> {
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

/// Primitive (pad-frame or board-frame) shape outline pieces.
pub fn graphic_pieces(g: &PcbGraphic, map: &dyn Fn(Pt) -> Pt) -> Vec<Piece> {
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
pub fn pad_pieces(pad: &PcbPad) -> Vec<Piece> {
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

/// Drill hole as a filled piece (stadium for slots).
pub fn drill_piece(pad: &PcbPad) -> Option<Piece> {
    let drill = pad.drill.as_ref()?;
    let map = |q: Pt| {
        let r = rotate(q, pad.angle);
        [pad.pos[0] + r[0], pad.pos[1] + r[1]]
    };
    if !drill.oval || (drill.size[0] - drill.size[1]).abs() < 1e-9 {
        return Some(Piece::Circle(pad.pos, drill.size[0] / 2.0));
    }
    Some(Piece::Polygon(
        stadium(drill.size).into_iter().map(map).collect(),
    ))
}

/// Shoelace area (positive for increasing-angle order in y-down space).
pub fn signed_area(pts: &[Pt]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

// ------------------------------------------------------------- hit testing

pub fn point_in_polygon(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx * dx + dy * dy;
    let t = if len == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
    (p[0] - cx).hypot(p[1] - cy)
}

/// Distance from `p` to a track/graphic centre line.
pub fn dist_to_shape(p: Pt, shape: &PcbShape) -> f64 {
    match shape {
        PcbShape::Segment { a, b } => dist_to_segment(p, *a, *b),
        PcbShape::Arc { c, r, start, end } => {
            let ang = (p[1] - c[1]).atan2(p[0] - c[0]);
            let off = (ang - start).rem_euclid(TAU);
            if off <= end - start {
                ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs()
            } else {
                let e0 = [c[0] + r * start.cos(), c[1] + r * start.sin()];
                let e1 = [c[0] + r * end.cos(), c[1] + r * end.sin()];
                (p[0] - e0[0])
                    .hypot(p[1] - e0[1])
                    .min((p[0] - e1[0]).hypot(p[1] - e1[1]))
            }
        }
        PcbShape::Circle { c, r } => ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs(),
        PcbShape::Polygon { pts } | PcbShape::Polyline { pts } => {
            let closed = matches!(shape, PcbShape::Polygon { .. });
            let n = pts.len();
            let segs = if closed { n } else { n.saturating_sub(1) };
            (0..segs)
                .map(|i| dist_to_segment(p, pts[i], pts[(i + 1) % n]))
                .fold(f64::MAX, f64::min)
        }
    }
}

pub fn piece_contains(piece: &Piece, p: Pt) -> bool {
    match piece {
        Piece::Circle(c, r) => (p[0] - c[0]).hypot(p[1] - c[1]) <= *r,
        Piece::Polygon(pts) => point_in_polygon(p, pts),
        Piece::Stroke(pts, w) => pts
            .windows(2)
            .any(|s| dist_to_segment(p, s[0], s[1]) <= w / 2.0),
    }
}

/// What a click landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Pad(usize, usize),
    Via(usize),
    Track(usize),
    Footprint(usize),
    Zone(usize),
}

/// Pick the top-most item at board point `p`. `visible(layer)` filters by
/// layer visibility; `front` is the layer index on top (F.Cu, or B.Cu when
/// flipped); `slop` is the pick tolerance in mm.
pub fn hit_test(
    board: &PcbBoard,
    p: Pt,
    slop: f64,
    visible: &dyn Fn(u16) -> bool,
    front: u16,
    pours: bool,
) -> Option<Hit> {
    let side_rank = |layers: &[u16]| if layers.contains(&front) { 0 } else { 1 };
    let mut best: Option<(u8, Hit)> = None;
    for (fi, fp) in board.footprints.iter().enumerate() {
        for (pi, pad) in fp.pads.iter().enumerate() {
            if !pad.layers.iter().any(|l| visible(*l)) {
                continue;
            }
            let pieces = pad_pieces(pad);
            if pieces.iter().any(|piece| piece_contains(piece, p)) {
                offer(&mut best, side_rank(&pad.layers), Hit::Pad(fi, pi));
            }
        }
    }
    for (i, via) in board.vias.iter().enumerate() {
        if via.layers.iter().any(|l| visible(*l))
            && (p[0] - via.pos[0]).hypot(p[1] - via.pos[1]) <= via.size / 2.0 + slop
        {
            offer(&mut best, 2, Hit::Via(i));
        }
    }
    for (i, t) in board.tracks.iter().enumerate() {
        if visible(t.layer) && dist_to_shape(p, &t.shape) <= t.width / 2.0 + slop {
            offer(
                &mut best,
                if t.layer == front { 3 } else { 4 },
                Hit::Track(i),
            );
        }
    }
    if best.is_some() {
        return best.map(|(_, h)| h);
    }
    for (i, fp) in board.footprints.iter().enumerate() {
        if point_in_polygon(p, &fp.bbox) {
            offer(
                &mut best,
                if fp.layer == front { 5 } else { 6 },
                Hit::Footprint(i),
            );
        }
    }
    if best.is_none() && pours {
        for (i, z) in board.zones.iter().enumerate().rev() {
            if z.fills
                .iter()
                .any(|f| visible(f.layer) && point_in_polygon(p, &f.pts))
            {
                offer(&mut best, 7, Hit::Zone(i));
            }
        }
    }
    best.map(|(_, h)| h)
}

fn offer(best: &mut Option<(u8, Hit)>, rank: u8, hit: Hit) {
    if best.is_none_or(|(r, _)| rank < r) {
        *best = Some((rank, hit));
    }
}

/// Net of a hit item (0 = none).
pub fn hit_net(board: &PcbBoard, hit: Hit) -> u32 {
    match hit {
        Hit::Pad(f, p) => board.footprints[f].pads[p].net,
        Hit::Via(i) => board.vias[i].net,
        Hit::Track(i) => board.tracks[i].net,
        Hit::Zone(i) => board.zones[i].net,
        Hit::Footprint(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::pcb::PcbDrill;

    fn pad(shape: &str, size: Pt, angle: f64) -> PcbPad {
        PcbPad {
            shape: shape.into(),
            size,
            angle,
            pos: [10.0, 10.0],
            ..PcbPad::default()
        }
    }

    #[test]
    fn view_roundtrip_and_fit() {
        let mut v = View::fit([0.0, 0.0, 100.0, 50.0], 1000.0, 1000.0, false);
        assert!((v.scale - 7.8).abs() < 1e-9);
        let s = v.to_screen([50.0, 25.0]);
        assert!((s[0] - 500.0).abs() < 1e-9 && (s[1] - 500.0).abs() < 1e-9);
        v.zoom_at([100.0, 100.0], 2.0);
        let b = v.to_board([100.0, 100.0]);
        let back = v.to_screen(b);
        assert!((back[0] - 100.0).abs() < 1e-9);
        v.flip_at([500.0, 500.0]);
        let b2 = v.to_board([500.0, 500.0]);
        assert!((v.to_screen(b2)[0] - 500.0).abs() < 1e-9);
        assert!(v.to_screen([b2[0] + 1.0, b2[1]])[0] < 500.0, "mirrored");
    }

    #[test]
    fn rotated_rect_pad_contains() {
        // 4x1 pad rotated 90: tall in y.
        let p = pad("rect", [4.0, 1.0], 90.0);
        let pieces = pad_pieces(&p);
        assert!(pieces.iter().any(|x| piece_contains(x, [10.0, 11.8])));
        assert!(!pieces.iter().any(|x| piece_contains(x, [11.8, 10.0])));
    }

    #[test]
    fn roundrect_and_chamfer_outlines() {
        let o = rect_outline([2.0, 1.0], 0.25, 0, 0.0);
        assert!(signed_area(&o) > 0.0);
        let full = 2.0;
        let rounded = signed_area(&o);
        assert!(rounded < full && rounded > full - 0.1);
        let ch = rect_outline([2.0, 2.0], 0.0, 1 | 8, 0.5);
        assert!((signed_area(&ch) - (4.0 - 0.25)).abs() < 1e-9);
        assert!(!point_in_polygon([-0.95, -0.95], &ch));
        assert!(point_in_polygon([0.95, -0.95], &ch));
    }

    #[test]
    fn oval_offset_and_drill() {
        let mut p = pad("oval", [1.7, 2.2], 0.0);
        p.offset = [0.0, 0.1];
        p.drill = Some(PcbDrill {
            oval: true,
            size: [0.8, 1.2],
        });
        let pieces = pad_pieces(&p);
        assert!(pieces.iter().any(|x| piece_contains(x, [10.0, 11.15])));
        assert!(!pieces.iter().any(|x| piece_contains(x, [10.0, 8.85])));
        let hole = drill_piece(&p).unwrap();
        assert!(piece_contains(&hole, [10.0, 10.55]));
        assert!(!piece_contains(&hole, [10.45, 10.0]));
    }

    #[test]
    fn arc_distance_respects_sweep() {
        let arc = PcbShape::Arc {
            c: [0.0, 0.0],
            r: 1.0,
            start: 0.0,
            end: PI / 2.0,
        };
        assert!(dist_to_shape([0.0, 1.0], &arc) < 1e-9);
        assert!(dist_to_shape([0.0, -1.0], &arc) > 1.0);
    }
}
