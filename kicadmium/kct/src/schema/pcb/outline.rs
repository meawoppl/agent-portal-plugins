//! Board outline extraction, `Edge.Cuts` contour editing, and `page_fit`.

use std::collections::BTreeSet;

use anyhow::anyhow;

use crate::core::board_outline::{board_outline_bounds, is_degenerate_closed_curve};
use crate::sexp::SExp;
use crate::Result;

use super::edit::build_board_outline_sexp;
use super::models::{arc_points_from_sexp, BoardGraphic, EdgeContour, GraphicArc, GraphicLine};
use super::util::{atom, gs, list, xy};
use super::{is_footprint_tag, Pcb, Point};

/// Bounding-box area (mm^2) below which a contour is a mounting hole.
pub const MOUNTING_HOLE_AREA_THRESHOLD: f64 = 25.0;

type Seg = (Point, Point);

fn points_close(a: Point, b: Point, tol: f64) -> bool {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy < tol * tol
}

fn rect_to_segments(s: Point, e: Point) -> Vec<Seg> {
    let (x1, y1) = s;
    let (x2, y2) = e;
    vec![
        ((x1, y1), (x2, y1)),
        ((x2, y1), (x2, y2)),
        ((x2, y2), (x1, y2)),
        ((x1, y2), (x1, y1)),
    ]
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra] = rb;
        }
    }
}

/// Groups of segment indices connected by endpoint proximity, in order of
/// first member.
fn group_segments_by_connectivity(segments: &[Seg], tol: f64) -> Vec<Vec<usize>> {
    let n = segments.len();
    let mut uf = UnionFind::new(n);
    for i in 0..n {
        let (si, ei) = segments[i];
        for (j, &(sj, ej)) in segments.iter().enumerate().skip(i + 1) {
            if points_close(si, sj, tol)
                || points_close(si, ej, tol)
                || points_close(ei, sj, tol)
                || points_close(ei, ej, tol)
            {
                uf.union(i, j);
            }
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        let r = uf.find(i);
        match roots.iter().position(|&x| x == r) {
            Some(k) => groups[k].push(i),
            None => {
                roots.push(r);
                groups.push(vec![i]);
            }
        }
    }
    groups
}

/// Chain one component into a closed, nondegenerate polygon (empty when
/// the component is open, branched, or encloses no area).
fn chain_segment_indices(segments: &[Seg], indices: &[usize], tol: f64) -> Vec<Point> {
    if indices.len() < 3 {
        return vec![];
    }
    for &i in indices {
        let (start, end) = segments[i];
        if points_close(start, end, tol) {
            return vec![];
        }
        for p in [start, end] {
            let mates = indices
                .iter()
                .filter(|&&j| j != i)
                .flat_map(|&j| [segments[j].0, segments[j].1])
                .filter(|&q| points_close(p, q, tol))
                .count();
            if mates != 1 {
                return vec![];
            }
        }
    }
    let first = indices[0];
    let mut polygon = vec![segments[first].0, segments[first].1];
    let mut remaining: BTreeSet<usize> = indices.iter().copied().filter(|&i| i != first).collect();
    while !remaining.is_empty() {
        let cur = *polygon.last().unwrap();
        let mut found = None;
        for &i in &remaining {
            let (s, e) = segments[i];
            if points_close(cur, s, tol) {
                found = Some((i, e));
                break;
            } else if points_close(cur, e, tol) {
                found = Some((i, s));
                break;
            }
        }
        match found {
            Some((i, p)) => {
                polygon.push(p);
                remaining.remove(&i);
            }
            None => break,
        }
    }
    if !remaining.is_empty() || !points_close(*polygon.last().unwrap(), polygon[0], tol) {
        return vec![];
    }
    let n = polygon.len();
    polygon[n - 1] = polygon[0];
    let (ox, oy) = polygon[0];
    let twice: f64 = polygon
        .windows(2)
        .map(|w| (w[0].0 - ox) * (w[1].1 - oy) - (w[1].0 - ox) * (w[0].1 - oy))
        .sum();
    if twice.abs() <= tol * tol {
        return vec![];
    }
    polygon
}

fn on_edge_cuts(node: &SExp) -> bool {
    node.find_child("layer")
        .and_then(|l| gs(l, 0))
        .is_some_and(|l| l == "Edge.Cuts")
}

fn collect_edge_points(nodes: &[&SExp]) -> Vec<Point> {
    let mut points = Vec::new();
    for node in nodes {
        if node.has_tag("gr_arc") {
            let (s, m, e) = arc_points_from_sexp(node);
            points.extend([s, m, e]);
            continue;
        }
        for tag in ["start", "end", "mid", "center"] {
            if let Some(n) = node.find(tag) {
                points.push(xy(n));
            }
        }
    }
    points
}

/// `round(value, 6)` as KiCad nm-grid text, trailing zeros stripped.
pub(crate) fn format_coord_mm(value: f64) -> String {
    let text = format!("{:.6}", (value * 1e6).round() / 1e6);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text {
        "" | "-0" => "0".into(),
        t => t.into(),
    }
}

fn translate_coord_node(node: &mut SExp, dx_nm: i64, dy_nm: i64) {
    let (Some(x), Some(y)) = (node.float_at(0), node.float_at(1)) else {
        return;
    };
    node.set_value(0, format_coord_mm(x + dx_nm as f64 / 1e6));
    node.set_value(1, format_coord_mm(y + dy_nm as f64 / 1e6));
}

fn translate_item_sexp(node: &mut SExp, dx_nm: i64, dy_nm: i64) {
    if matches!(
        node.tag(),
        Some("at" | "start" | "end" | "mid" | "center" | "xy")
    ) {
        translate_coord_node(node, dx_nm, dy_nm);
    }
    for child in node.children.iter_mut().filter(|c| c.is_list()) {
        translate_item_sexp(child, dx_nm, dy_nm);
    }
}

impl Pcb {
    /// `gr_poly`/`gr_curve` `Edge.Cuts` vertex chains (sheet-absolute);
    /// closed collinear cubics are skipped.
    fn edge_cuts_poly_chains(&self) -> Vec<Vec<Point>> {
        let mut chains = Vec::new();
        for child in self.doc.root.children.iter().filter(|c| c.is_list()) {
            if !(child.has_tag("gr_poly") || child.has_tag("gr_curve")) || !on_edge_cuts(child) {
                continue;
            }
            let mut chain = Vec::new();
            for pts in child.find_all("pts") {
                for p in pts.children_named("xy") {
                    if let (Some(x), Some(y)) = (p.float_at(0), p.float_at(1)) {
                        chain.push((x, y));
                    }
                }
            }
            if child.has_tag("gr_curve") && is_degenerate_closed_curve(&chain) {
                continue;
            }
            if !chain.is_empty() {
                chains.push(chain);
            }
        }
        chains
    }

    /// Ordered board outline polygon (board-relative): the largest closed
    /// nondegenerate contour formed by `Edge.Cuts` lines, arcs (start-mid-end
    /// chords), rects, polys and curves, stitched with 0.01 mm tolerance.
    pub fn get_board_outline(&self) -> Vec<Point> {
        let mut segments: Vec<Seg> = Vec::new();
        for l in self.graphic_lines.iter().filter(|l| l.layer == "Edge.Cuts") {
            segments.push((l.start, l.end));
        }
        for a in self.graphic_arcs.iter().filter(|a| a.layer == "Edge.Cuts") {
            segments.push((a.start, a.mid));
            segments.push((a.mid, a.end));
        }
        for g in self
            .graphics
            .iter()
            .filter(|g| g.layer == "Edge.Cuts" && g.graphic_type == "rect")
        {
            segments.extend(rect_to_segments(g.start, g.end));
        }
        for chain in self.edge_cuts_poly_chains() {
            for w in chain.windows(2) {
                segments.push((w[0], w[1]));
            }
            if chain.len() >= 3 {
                segments.push((*chain.last().unwrap(), chain[0]));
            }
        }
        if segments.is_empty() {
            return vec![];
        }
        let tol = 0.01;
        let mut best: Vec<Point> = Vec::new();
        let mut best_area = -1.0;
        for group in group_segments_by_connectivity(&segments, tol) {
            let cand = chain_segment_indices(&segments, &group, tol);
            if cand.is_empty() {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for &(x, y) in &cand {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            let area = (x1 - x0) * (y1 - y0);
            if area > best_area {
                best_area = area;
                best = cand;
            }
        }
        let (ox, oy) = self.board_origin;
        if ox != 0.0 || oy != 0.0 {
            for p in &mut best {
                p.0 -= ox;
                p.1 -= oy;
            }
        }
        best
    }

    /// Every `Edge.Cuts` line/arc/rect as straight segments (arcs as two
    /// chords through `mid`), board-relative.
    pub fn get_board_outline_segments(&self) -> Vec<Seg> {
        let mut segments: Vec<Seg> = Vec::new();
        for l in self.graphic_lines.iter().filter(|l| l.layer == "Edge.Cuts") {
            segments.push((l.start, l.end));
        }
        for a in self.graphic_arcs.iter().filter(|a| a.layer == "Edge.Cuts") {
            segments.push((a.start, a.mid));
            segments.push((a.mid, a.end));
        }
        for g in self
            .graphics
            .iter()
            .filter(|g| g.layer == "Edge.Cuts" && g.graphic_type == "rect")
        {
            segments.extend(rect_to_segments(g.start, g.end));
        }
        let (ox, oy) = self.board_origin;
        if ox != 0.0 || oy != 0.0 {
            for (a, b) in &mut segments {
                *a = (a.0 - ox, a.1 - oy);
                *b = (b.0 - ox, b.1 - oy);
            }
        }
        segments
    }

    /// Group `Edge.Cuts` lines/arcs (chained at 0.001 mm) and standalone
    /// rects/circles into contours; small ones are flagged mounting holes.
    pub fn list_edge_contours(&self) -> Vec<EdgeContour> {
        let mut elements: Vec<(usize, &SExp, Vec<Point>)> = Vec::new();
        for (idx, child) in self.doc.root.children.iter().enumerate() {
            if child.is_atom() {
                continue;
            }
            let tag = child.tag().unwrap_or("");
            if !matches!(tag, "gr_line" | "gr_arc" | "gr_rect" | "gr_circle") {
                continue;
            }
            let Some(layer) = child.find("layer") else {
                continue;
            };
            if gs(layer, 0).unwrap_or_default() != "Edge.Cuts" {
                continue;
            }
            let endpoints = match tag {
                "gr_arc" => {
                    let (s, _, e) = arc_points_from_sexp(child);
                    vec![s, e]
                }
                "gr_line" => match (child.find("start"), child.find("end")) {
                    (Some(s), Some(e)) => vec![xy(s), xy(e)],
                    _ => vec![],
                },
                _ => vec![],
            };
            elements.push((idx, child, endpoints));
        }
        if elements.is_empty() {
            return vec![];
        }
        let mut standalone: Vec<Vec<usize>> = Vec::new();
        let mut chainable: Vec<usize> = Vec::new();
        for (k, (_, node, _)) in elements.iter().enumerate() {
            if node.has_tag("gr_rect") || node.has_tag("gr_circle") {
                standalone.push(vec![k]);
            } else {
                chainable.push(k);
            }
        }
        let mut uf = UnionFind::new(elements.len());
        let tol = 0.001;
        for (pos, &i) in chainable.iter().enumerate() {
            for &j in &chainable[pos + 1..] {
                let close = elements[i]
                    .2
                    .iter()
                    .any(|&a| elements[j].2.iter().any(|&b| points_close(a, b, tol)));
                if close {
                    uf.union(i, j);
                }
            }
        }
        let mut roots: Vec<usize> = Vec::new();
        let mut chain_groups: Vec<Vec<usize>> = Vec::new();
        for &i in &chainable {
            let r = uf.find(i);
            match roots.iter().position(|&x| x == r) {
                Some(k) => chain_groups[k].push(i),
                None => {
                    roots.push(r);
                    chain_groups.push(vec![i]);
                }
            }
        }
        let mut contours = Vec::new();
        for group in standalone.into_iter().chain(chain_groups) {
            let nodes: Vec<&SExp> = group.iter().map(|&k| elements[k].1).collect();
            let pts = collect_edge_points(&nodes);
            if pts.is_empty() {
                continue;
            }
            let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for (x, y) in pts {
                b.0 = b.0.min(x);
                b.1 = b.1.min(y);
                b.2 = b.2.max(x);
                b.3 = b.3.max(y);
            }
            let area = (b.2 - b.0) * (b.3 - b.1);
            contours.push(EdgeContour {
                index: contours.len(),
                element_count: nodes.len(),
                bbox: b,
                is_mounting_hole: area < MOUNTING_HOLE_AREA_THRESHOLD,
                node_indices: group.iter().map(|&k| elements[k].0).collect(),
            });
        }
        contours
    }

    fn remove_root_children(&mut self, indices: &[usize]) {
        let mut sorted = indices.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        for i in sorted.into_iter().rev() {
            if i < self.doc.root.children.len() {
                self.doc.root.children.remove(i);
            }
        }
    }

    fn rebuild_graphics(&mut self) {
        self.graphic_lines.clear();
        self.graphic_arcs.clear();
        self.graphics.clear();
        for child in self.doc.root.children.iter().filter(|c| c.is_list()) {
            match child.tag() {
                Some("gr_line") => {
                    self.graphic_lines.push(GraphicLine::from_sexp(child));
                    self.graphics.push(BoardGraphic::from_sexp(child, "line"));
                }
                Some("gr_arc") => {
                    self.graphic_arcs.push(GraphicArc::from_sexp(child));
                    self.graphics.push(BoardGraphic::from_sexp(child, "arc"));
                }
                Some(t @ ("gr_rect" | "gr_circle" | "gr_poly")) => {
                    self.graphics.push(BoardGraphic::from_sexp(child, &t[3..]));
                }
                _ => {}
            }
        }
    }

    /// Remove the contour numbered `index` (from [`Pcb::list_edge_contours`]).
    pub fn remove_edge_contour(&mut self, index: usize) -> bool {
        let Some(target) = self
            .list_edge_contours()
            .into_iter()
            .find(|c| c.index == index)
        else {
            return false;
        };
        let uuids: BTreeSet<String> = target
            .node_indices
            .iter()
            .filter_map(|&i| {
                self.doc.root.children[i]
                    .find("uuid")
                    .and_then(|u| gs(u, 0))
            })
            .filter(|u| !u.is_empty())
            .collect();
        self.remove_root_children(&target.node_indices);
        if uuids.is_empty() {
            self.rebuild_graphics();
        } else {
            self.graphic_lines.retain(|g| !uuids.contains(&g.uuid));
            self.graphic_arcs.retain(|g| !uuids.contains(&g.uuid));
            self.graphics.retain(|g| !uuids.contains(&g.uuid));
        }
        true
    }

    /// Replace every non-mounting-hole contour with a `gr_line` rectangle at
    /// sheet-absolute `(origin_x, origin_y)`. Returns contours removed.
    pub fn replace_outline(
        &mut self,
        origin_x: f64,
        origin_y: f64,
        width: f64,
        height: f64,
    ) -> usize {
        let contours = self.list_edge_contours();
        let mut doomed = Vec::new();
        let mut removed = 0;
        for c in contours.iter().filter(|c| !c.is_mounting_hole) {
            doomed.extend(c.node_indices.iter().copied());
            removed += 1;
        }
        self.remove_root_children(&doomed);
        for line in build_board_outline_sexp(width, height, origin_x, origin_y) {
            self.doc.root.push(line);
        }
        self.rebuild_graphics();
        removed
    }

    /// Resize the sheet to `(paper "User" W H)` fitting the board with a
    /// uniform `margin`, rigidly translating every item on the nm grid so
    /// the outline sits at `(margin, margin)`. Returns the new paper size.
    pub fn page_fit(&mut self, margin: f64) -> Result<Point> {
        let bbox = board_outline_bounds(&self.doc.root)
            .map_err(|e| anyhow!(e))?
            .ok_or_else(|| {
                anyhow!("page_fit() requires an Edge.Cuts board outline; none found.")
            })?;
        let (min_x, min_y, max_x, max_y) = bbox;
        let dx_nm = ((margin - min_x) * 1e6).round() as i64;
        let dy_nm = ((margin - min_y) * 1e6).round() as i64;
        for child in self.doc.root.children.iter_mut().filter(|c| c.is_list()) {
            let tag = child.tag();
            if is_footprint_tag(tag) {
                if let Some(at) = child.get_mut("at") {
                    translate_coord_node(at, dx_nm, dy_nm);
                }
            } else if matches!(
                tag,
                Some(
                    "segment"
                        | "via"
                        | "arc"
                        | "zone"
                        | "gr_line"
                        | "gr_arc"
                        | "gr_rect"
                        | "gr_circle"
                        | "gr_poly"
                        | "gr_curve"
                        | "gr_text"
                        | "text"
                        | "dimension"
                        | "target"
                )
            ) {
                translate_item_sexp(child, dx_nm, dy_nm);
            }
        }
        let round6 = |v: f64| (v * 1e6).round() / 1e6;
        let new_w = round6(max_x - min_x + 2.0 * margin);
        let new_h = round6(max_y - min_y + 2.0 * margin);
        let root = &mut self.doc.root;
        root.remove_child("paper");
        let paper = list(
            "paper",
            vec![SExp::quoted("User"), atom(new_w), atom(new_h)],
        );
        match root.children.iter().position(|c| c.has_tag("version")) {
            Some(i) => root.children.insert(i + 1, paper),
            None => root.insert(0, paper),
        }
        self.reparse()?;
        Ok((new_w, new_h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coord_format() {
        assert_eq!(format_coord_mm(148.0), "148");
        assert_eq!(format_coord_mm(147.9252), "147.9252");
        assert_eq!(format_coord_mm(-0.0000001), "0");
        assert_eq!(format_coord_mm(0.1 + 0.2), "0.3");
    }
}
