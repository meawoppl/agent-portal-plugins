//! Net-independent physical copper slit detector (port of
//! `kicad_tools.validate.rules.physical_gap`): union every copper source per
//! layer and report facing boundary edges closer than a minimum gap that do
//! not belong to joined copper.

use std::f64::consts::TAU;

use super::clearance::{pad_on_layer, pad_polygon, repair_fill_polygon};
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::{fill_token_is_filled, Footprint, FootprintGraphic, Pcb};
use crate::sexp::{SExp, Token, Value};
use crate::validate::violations::{DRCResults, DRCViolation};

pub const TOLERANCE: f64 = 0.001;

#[derive(Debug, Clone)]
pub struct CopperSource {
    pub geometry: Geom,
    pub layer: String,
    pub net: String,
    pub identity: String,
}

type C = (f64, f64);

/// `_arc_geometry`: flattened start/mid/end arc (<= 0.1 um chord error),
/// buffered by half its width.
fn arc_geometry(node: &SExp) -> Result<Geom, String> {
    let mut pts = Vec::new();
    for key in ["start", "mid", "end"] {
        let Some(p) = node.get(key) else {
            return Err("unsupported legacy or incomplete copper arc".into());
        };
        let (Some(x), Some(y)) = (p.float_at(0), p.float_at(1)) else {
            return Err("unsupported legacy or incomplete copper arc".into());
        };
        pts.push((x, y));
    }
    let ((x1, y1), (x2, y2), (x3, y3)) = (pts[0], pts[1], pts[2]);
    let det = 2.0 * (x1 * (y2 - y3) + x2 * (y3 - y1) + x3 * (y1 - y2));
    if det.abs() < 1e-12 {
        return Err("degenerate copper arc".into());
    }
    let (q1, q2, q3) = (x1 * x1 + y1 * y1, x2 * x2 + y2 * y2, x3 * x3 + y3 * y3);
    let cx = (q1 * (y2 - y3) + q2 * (y3 - y1) + q3 * (y1 - y2)) / det;
    let cy = (q1 * (x3 - x2) + q2 * (x1 - x3) + q3 * (x2 - x1)) / det;
    let r = crate::utils::pymath::hypot(x1 - cx, y1 - cy);
    let ang: Vec<f64> = pts.iter().map(|(x, y)| (y - cy).atan2(x - cx)).collect();
    let mut sweep = (ang[2] - ang[0]).rem_euclid(TAU);
    if (ang[1] - ang[0]).rem_euclid(TAU) > sweep {
        sweep -= TAU;
    }
    let step = 2.0 * (1.0 - 0.0001 / r).max(-1.0).acos();
    let count = ((sweep.abs() / step.max(1e-6)).ceil() as usize).max(2);
    let width = node.get("width").and_then(|w| w.float_at(0));
    let Some(width) = width.filter(|w| *w > 0.0) else {
        return Err("missing copper arc width".into());
    };
    let line: Vec<C> = (0..=count)
        .map(|i| {
            let a = ang[0] + sweep * i as f64 / count as f64;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect();
    Ok(sh::buffer_line_q(&line, width / 2.0, 64))
}

/// `_footprint_transform`: footprint-local -> board coordinates.
fn footprint_transform(fp: &Footprint) -> impl Fn(C) -> C {
    let (ox, oy) = fp.position;
    let a = (-fp.rotation).to_radians();
    let (c, s) = (a.cos(), a.sin());
    move |(x, y)| (ox + x * c - y * s, oy + x * s + y * c)
}

fn fp_poly_geometry(g: &FootprintGraphic, t: &dyn Fn(C) -> C) -> Option<Geom> {
    let pts: Vec<C> = g.points.iter().map(|p| t(*p)).collect();
    if pts.len() < 3 {
        return None;
    }
    let w = g.stroke_width;
    if g.is_filled() {
        let poly = repair_fill_polygon(&Poly::new(pts));
        return Some(if w > 0.0 {
            sh::buffer_polygon_q(&poly, w / 2.0, 64)
        } else {
            poly
        });
    }
    if w <= 0.0 {
        return None;
    }
    let mut ring = pts.clone();
    ring.push(pts[0]);
    Some(sh::buffer_line_q(&ring, w / 2.0, 64))
}

fn floats(node: &SExp) -> Vec<Option<f64>> {
    node.children.iter().map(|c| c.value.as_ref().and_then(Value::as_f64)).collect()
}

fn fp_poly_issues(node: &SExp, layers: &[String]) -> Vec<String> {
    let name = node.get("layer").and_then(|l| l.text_at(0));
    let Some(name) = name.filter(|n| n.ends_with(".Cu")) else {
        return vec![];
    };
    if !layers.contains(&name) {
        return vec![format!("unresolved fp_poly copper layer: {name}")];
    }
    let mut issues = Vec::new();
    let verts: Vec<&SExp> = node
        .get("pts")
        .map(|p| p.children.iter().filter(|c| c.has_tag("xy")).collect())
        .unwrap_or_default();
    if verts.len() < 3 {
        issues.push("invalid fp_poly points: at least three vertices required".to_string());
    }
    for xy in &verts {
        let v = floats(xy);
        if v.len() != 2 || v.iter().any(|x| !x.is_some_and(f64::is_finite)) {
            issues.push("invalid fp_poly coordinates: finite numeric geometry required".into());
            break;
        }
    }
    let width_node = node.get("stroke").and_then(|s| s.get("width"));
    let mut width = match width_node {
        Some(w) => w.float_at(0),
        None => Some(0.0),
    };
    if !width.is_some_and(|w| w.is_finite() && w >= 0.0) {
        issues.push("invalid fp_poly stroke: finite non-negative width required".into());
        width = Some(0.0);
    }
    let token = node
        .get("fill")
        .and_then(|f| f.text_at(0))
        .unwrap_or_default();
    if !fill_token_is_filled(&token) && width.unwrap_or(0.0) <= 0.0 {
        issues.push("unsupported fp_poly: neither filled nor stroked copper".into());
    }
    issues
}

fn raw_copper_graphic_issues(pcb: &Pcb) -> Vec<String> {
    const TEXT: [&str; 5] = ["gr_text", "fp_text", "property", "gr_text_box", "fp_text_box"];
    let hidden = |node: &SExp| -> bool {
        let mut containers = vec![(node, true)];
        if let Some(e) = node.get("effects") {
            containers.push((e, false));
        }
        for (c, is_node) in containers {
            let tag = node.name.as_deref().unwrap_or("");
            let prefix = if is_node {
                if tag == "fp_text" || tag == "property" {
                    2
                } else {
                    1
                }
            } else {
                0
            };
            for child in c.children.iter().skip(prefix) {
                let atom_hide = child.is_atom()
                    && matches!(&child.value, Some(Value::Str(s)) if s == "hide")
                    && child.token != Token::Quoted;
                if atom_hide {
                    return true;
                }
                if child.has_tag("hide")
                    && (child.children.is_empty() || child.text_at(0).as_deref() == Some("yes"))
                {
                    return true;
                }
            }
        }
        false
    };
    let mut issues = Vec::new();
    let mut inspect = |container: &SExp, supported: &[&str]| {
        let cname = container.name.as_deref().unwrap_or("");
        for (i, node) in container.children.iter().enumerate() {
            if node.is_atom() || supported.iter().any(|s| node.has_tag(s)) {
                continue;
            }
            let copper = node
                .children
                .iter()
                .filter(|c| c.has_tag("layer") || c.has_tag("layers"))
                .flat_map(|l| l.children.iter())
                .any(|a| a.is_atom() && matches!(&a.value, Some(Value::Str(s)) if s.ends_with(".Cu")));
            if !copper {
                continue;
            }
            let nname = node.name.as_deref().unwrap_or("");
            if TEXT.contains(&nname) && hidden(node) {
                continue;
            }
            issues.push(format!("unsupported copper graphic: {cname}/{nname}:{i}"));
        }
    };
    let root = pcb.sexp();
    inspect(root, &["segment", "arc", "via", "zone", "footprint", "module"]);
    for node in &root.children {
        if node.has_tag("footprint") || node.has_tag("module") {
            inspect(node, &["pad", "zone", "fp_poly"]);
        }
    }
    issues
}

fn raw_geometry_issues(pcb: &Pcb) -> Vec<String> {
    let layers: Vec<String> = pcb.copper_layers().iter().map(|l| l.name.clone()).collect();
    let mut issues: Vec<String> = Vec::new();
    let numeric = |issues: &mut Vec<String>, node: &SExp, tag: &str, lengths: &[usize], required: bool, positive: bool| {
        let fields: Vec<&SExp> = node.children.iter().filter(|c| c.has_tag(tag)).collect();
        if fields.is_empty() && !required {
            return;
        }
        let nname = node.name.as_deref().unwrap_or("");
        if fields.len() != 1 {
            issues.push(format!("invalid {nname} {tag}: missing or repeated field"));
            return;
        }
        let f = fields[0];
        let vals = floats(f);
        let bad = !lengths.contains(&vals.len())
            || f.children.iter().zip(&vals).any(|(c, v)| {
                !c.is_atom() || !v.is_some_and(|v| v.is_finite() && !(positive && v <= 0.0))
            });
        if bad {
            issues.push(format!("invalid {nname} {tag}: finite numeric geometry required"));
        }
    };
    for node in &pcb.sexp().children {
        match node.name.as_deref() {
            Some(t @ ("segment" | "arc")) => {
                numeric(&mut issues, node, "start", &[2], true, false);
                numeric(&mut issues, node, "end", &[2], true, false);
                if t == "arc" {
                    numeric(&mut issues, node, "mid", &[2], true, false);
                }
                numeric(&mut issues, node, "width", &[1], true, true);
                let ok = node
                    .get("layer")
                    .and_then(|l| l.text_at(0))
                    .is_some_and(|l| layers.contains(&l));
                if !ok {
                    issues.push(format!("unresolved {t} copper layer"));
                }
            }
            Some("via") => {
                if node.get("padstack").is_some() {
                    issues.push("unsupported via padstack: layer-specific copper unresolved".into());
                }
                numeric(&mut issues, node, "at", &[2], true, false);
                numeric(&mut issues, node, "size", &[1], true, true);
                let span_ok = node.get("layers").is_some_and(|s| {
                    s.children.len() == 2
                        && s.children.iter().all(|c| {
                            matches!(&c.value, Some(Value::Str(v)) if layers.contains(v))
                        })
                });
                if !span_ok {
                    issues.push("unresolved via layer span".into());
                }
            }
            Some("footprint" | "module") => {
                numeric(&mut issues, node, "at", &[2, 3], false, false);
                for pad in node.find_all("pad") {
                    if pad.text_at(1).as_deref() == Some("np_thru_hole") {
                        continue;
                    }
                    if pad.get("padstack").is_some() {
                        issues.push("unsupported pad padstack: layer-specific copper unresolved".into());
                    }
                    numeric(&mut issues, pad, "at", &[2, 3], true, false);
                    numeric(&mut issues, pad, "size", &[2], true, true);
                    numeric(&mut issues, pad, "roundrect_rratio", &[1], false, false);
                    if let Some(r) = pad.get("roundrect_rratio") {
                        if !r.float_at(0).is_some_and(|v| (0.0..=0.5).contains(&v)) {
                            issues.push("invalid pad roundrect ratio".into());
                        }
                    }
                    if pad.get("chamfer").is_some() || pad.get("chamfer_ratio").is_some() {
                        issues.push("unsupported pad chamfer geometry".into());
                    }
                }
                for poly in node.find_all("fp_poly") {
                    issues.extend(fp_poly_issues(poly, &layers));
                }
            }
            Some("zone") => {
                for fill in node.find_all("filled_polygon") {
                    match fill.get("pts") {
                        None => issues.push("invalid filled polygon: missing points".into()),
                        Some(pts) => {
                            for xy in pts.children.iter().filter(|c| c.has_tag("xy")) {
                                let v = floats(xy);
                                if v.len() != 2 || v.iter().any(|x| !x.is_some_and(f64::is_finite)) {
                                    issues.push("invalid filled polygon coordinates".into());
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    issues
}

/// `_collect`: every copper source, or the reasons coverage is incomplete.
pub fn collect(pcb: &Pcb) -> (Vec<CopperSource>, Vec<String>) {
    let mut sources = Vec::new();
    let mut unsupported = raw_geometry_issues(pcb);
    unsupported.extend(raw_copper_graphic_issues(pcb));
    if !unsupported.is_empty() {
        return (sources, unsupported);
    }
    let layer_names: Vec<String> = pcb.copper_layers().iter().map(|l| l.name.clone()).collect();
    let net_name = |n: i64| {
        pcb.nets()
            .iter()
            .rev()
            .find(|x| x.number == n)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| n.to_string())
    };
    for (i, s) in pcb.segments().iter().enumerate() {
        sources.push(CopperSource {
            geometry: sh::segment_buffer_q(s.start, s.end, s.width / 2.0, 64),
            layer: s.layer.clone(),
            net: net_name(s.net_number),
            identity: if s.uuid.is_empty() { format!("segment:{i}") } else { s.uuid.clone() },
        });
    }
    for fp in pcb.footprints() {
        let t = footprint_transform(fp);
        for (i, g) in fp.graphics.iter().enumerate() {
            if g.graphic_type != "poly" || !layer_names.contains(&g.layer) {
                continue;
            }
            let identity = if g.uuid.is_empty() {
                format!("{}:fp_poly:{i}", fp.reference)
            } else {
                g.uuid.clone()
            };
            match fp_poly_geometry(g, &t) {
                Some(geom) if !geom.is_empty() => sources.push(CopperSource {
                    geometry: geom,
                    layer: g.layer.clone(),
                    net: String::new(),
                    identity,
                }),
                _ => unsupported.push(format!("degenerate fp_poly: {identity}")),
            }
        }
        for (i, pad) in fp.pads.iter().enumerate() {
            if pad.pad_type == "np_thru_hole" {
                continue;
            }
            if !["circle", "rect", "oval", "obround", "roundrect"].contains(&pad.shape.as_str()) {
                unsupported.push(format!("unsupported pad shape: {}:{}", fp.reference, pad.number));
                continue;
            }
            let Some(geom) = pad_polygon(pad, fp) else {
                unsupported.push(format!("degenerate pad: {}:{}", fp.reference, pad.number));
                continue;
            };
            for l in &layer_names {
                if pad_on_layer(pad, l) {
                    sources.push(CopperSource {
                        geometry: geom.clone(),
                        layer: l.clone(),
                        net: net_name(pad.net_number),
                        identity: if pad.uuid.is_empty() {
                            format!("{}:pad:{i}", fp.reference)
                        } else {
                            pad.uuid.clone()
                        },
                    });
                }
            }
        }
    }
    for (i, v) in pcb.vias().iter().enumerate() {
        let idx: Option<Vec<usize>> = v
            .layers
            .iter()
            .map(|l| layer_names.iter().position(|x| x == l))
            .collect();
        let Some(mut idx) = idx.filter(|x| !x.is_empty()) else {
            unsupported.push(format!("unresolved via layers: {}", v.uuid));
            continue;
        };
        idx.sort();
        let (first, last) = (idx[0], idx[idx.len() - 1]);
        for l in &layer_names[first..=last] {
            sources.push(CopperSource {
                geometry: sh::point_buffer_q(v.position, v.size / 2.0, 64),
                layer: l.clone(),
                net: net_name(v.net_number),
                identity: if v.uuid.is_empty() { format!("via:{i}") } else { v.uuid.clone() },
            });
        }
    }
    for (i, z) in pcb.zones().iter().enumerate() {
        if z.filled_polygons.is_empty() && z.keepout.is_none() {
            unsupported.push(format!(
                "unfilled zone: {}",
                if z.uuid.is_empty() { i.to_string() } else { z.uuid.clone() }
            ));
        }
        for (fi, pts) in z.filled_polygons.iter().enumerate() {
            if pts.len() >= 3 {
                sources.push(CopperSource {
                    geometry: repair_fill_polygon(&Poly::new(pts.clone())),
                    layer: z.filled_polygon_layer(fi).to_string(),
                    net: if z.net_name.is_empty() { net_name(z.net_number) } else { z.net_name.clone() },
                    identity: if z.uuid.is_empty() { format!("zone:{i}") } else { z.uuid.clone() },
                });
            }
        }
    }
    for (i, node) in pcb.sexp().children.iter().enumerate() {
        if !node.has_tag("arc") {
            continue;
        }
        let layer = node.get("layer").and_then(|l| l.text_at(0)).unwrap_or_default();
        if !layer_names.contains(&layer) {
            continue;
        }
        let geom = match arc_geometry(node) {
            Ok(g) => g,
            Err(e) => {
                unsupported.push(e);
                continue;
            }
        };
        let net = match node.get("net").and_then(|n| n.first_atom().cloned()) {
            Some(Value::Int(n)) => net_name(n),
            Some(v) => v.to_string(),
            None => net_name(0),
        };
        sources.push(CopperSource {
            geometry: geom,
            layer,
            net,
            identity: node
                .get("uuid")
                .and_then(|u| u.text_at(0))
                .unwrap_or_else(|| format!("arc:{i}")),
        });
    }
    (sources, unsupported)
}

/// `shapely.geometry.polygon.orient(p, sign=1)`: CCW shell, CW holes.
fn orient_ccw(p: &Poly) -> Poly {
    // `sh::ring_area` follows GEOS: positive for clockwise rings.
    let mut shell = p.shell.clone();
    if sh::ring_area(&shell) > 0.0 {
        shell.reverse();
    }
    let holes = p
        .holes
        .iter()
        .map(|h| {
            let mut h = h.clone();
            if sh::ring_area(&h) < 0.0 {
                h.reverse();
            }
            h
        })
        .collect();
    Poly { shell, holes }
}

fn seg_bounds(a: C, b: C) -> (f64, f64, f64, f64) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

/// Length of `[a, b]` inside the layer union (`connector.intersection(union)
/// .length`), splitting at boundary-edge crossings found via the edge tree.
fn inside_length(a: C, b: C, edges: &[(C, C)], tree: &StrTree, union: &sh::Prepared) -> f64 {
    let mut ts = vec![0.0, 1.0];
    for k in tree.query(seg_bounds(a, b)) {
        let (c, d) = edges[k];
        let denom = (b.0 - a.0) * (d.1 - c.1) - (b.1 - a.1) * (d.0 - c.0);
        if denom == 0.0 {
            continue;
        }
        let r = ((a.1 - c.1) * (d.0 - c.0) - (a.0 - c.0) * (d.1 - c.1)) / denom;
        let s = ((a.1 - c.1) * (b.0 - a.0) - (a.0 - c.0) * (b.1 - a.1)) / denom;
        if (0.0..=1.0).contains(&r) && (0.0..=1.0).contains(&s) {
            ts.push(r);
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup();
    let len = sh::dist(a, b);
    let at = |t: f64| (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
    ts.windows(2)
        .filter(|w| union.covers_point(at((w[0] + w[1]) / 2.0)))
        .map(|w| (w[1] - w[0]) * len)
        .sum()
}

/// `check_physical_copper_gap(pcb, minimum_mm)`.
pub fn check_physical_copper_gap(pcb: &Pcb, _design_rules: &DesignRules, minimum_mm: f64) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    if !minimum_mm.is_finite() || minimum_mm <= 0.0 {
        return results;
    }
    let (sources, unsupported) = collect(pcb);
    let mut uniq = unsupported.clone();
    uniq.sort();
    uniq.dedup();
    for d in uniq {
        results.add(DRCViolation::new(
            "physical_copper_gap_incomplete",
            "error",
            format!("Physical gap coverage incomplete: {d}"),
        ));
    }
    let mut layers: Vec<&String> = sources.iter().map(|s| &s.layer).collect();
    layers.sort();
    layers.dedup();
    for layer in layers {
        let local: Vec<&CopperSource> = sources
            .iter()
            .filter(|s| s.layer == *layer && !s.geometry.is_empty())
            .collect();
        let geoms: Vec<Geom> = local.iter().map(|s| s.geometry.clone()).collect();
        let union = sh::unary_union(&geoms);
        let mut edges: Vec<(C, C)> = Vec::new();
        let mut normals: Vec<C> = Vec::new();
        for poly in union.polys() {
            let p = orient_ccw(poly);
            for ring in std::iter::once(&p.shell).chain(p.holes.iter()) {
                for w in ring.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len = crate::utils::pymath::hypot(dx, dy);
                    if len <= 1e-12 {
                        continue;
                    }
                    edges.push((a, b));
                    normals.push((dy / len, -dx / len));
                }
            }
        }
        // shapely `STRtree` over sources and boundary edges (GEOS query order).
        let src_bounds: Vec<Option<sh::Bounds>> = local.iter().map(|s| s.geometry.bounds()).collect();
        let source_tree = StrTree::new(&src_bounds);
        // `dwithin(p, geom, TOL) and geom.boundary.distance(p) <= TOL`: the
        // boundary test implies the first, so only it is evaluated (on a
        // lazily prepared boundary index per source).
        let boundaries: Vec<std::cell::OnceCell<sh::Prepared>> =
            (0..local.len()).map(|_| std::cell::OnceCell::new()).collect();
        let owners_at = |p: C| -> Vec<usize> {
            let q = (p.0 - TOLERANCE, p.1 - TOLERANCE, p.0 + TOLERANCE, p.1 + TOLERANCE);
            source_tree
                .query(q)
                .into_iter()
                .filter(|&k| {
                    let prep = boundaries[k].get_or_init(|| {
                        let rings: Vec<Vec<C>> =
                            local[k].geometry.lines().iter().map(|l| l.to_vec()).collect();
                        sh::Prepared::new(Geom::Lines(rings))
                    });
                    sh::distance_prep(&Geom::Point(p), prep) <= TOLERANCE
                })
                .collect()
        };
        let mut owner_cache: std::collections::HashMap<(u64, u64), Vec<usize>> =
            std::collections::HashMap::new();
        let mut joint_cache: std::collections::HashMap<(usize, usize), Option<Geom>> =
            std::collections::HashMap::new();
        let eb: Vec<(f64, f64, f64, f64)> = edges.iter().map(|(a, b)| seg_bounds(*a, *b)).collect();
        let edge_tree = StrTree::new(&eb.iter().copied().map(Some).collect::<Vec<_>>());
        let prep_union = sh::Prepared::new(union.clone());
        let mut findings: Vec<((Vec<String>, Vec<String>), DRCViolation)> = Vec::new();
        for i in 0..edges.len() {
            let (ea, eb_) = edges[i];
            let bi = eb[i];
            let q = (bi.0 - minimum_mm, bi.1 - minimum_mm, bi.2 + minimum_mm, bi.3 + minimum_mm);
            let js: Vec<usize> = edge_tree.query(q);
            for j in js {
                if j <= i {
                    continue;
                }
                let (fa, fb) = edges[j];
                if sh::segment_to_segment(ea, eb_, fa, fb) > minimum_mm {
                    continue;
                }
                if normals[i].0 * normals[j].0 + normals[i].1 * normals[j].1 > -0.64 {
                    continue;
                }
                let mut cands = vec![sh::closest_points_segments(ea, eb_, fa, fb)];
                let mid_i = (ea.0 + 0.5 * (eb_.0 - ea.0), ea.1 + 0.5 * (eb_.1 - ea.1));
                cands.push((mid_i, sh::closest_point_on_segment(mid_i, fa, fb)));
                let mid_j = (fa.0 + 0.5 * (fb.0 - fa.0), fa.1 + 0.5 * (fb.1 - fa.1));
                cands.push((sh::closest_point_on_segment(mid_j, ea, eb_), mid_j));
                for (a, b) in cands {
                    let d = sh::dist(a, b);
                    if d <= TOLERANCE || d >= minimum_mm - TOLERANCE {
                        continue;
                    }
                    let (dx, dy) = ((b.0 - a.0) / d, (b.1 - a.1) / d);
                    if normals[i].0 * dx + normals[i].1 * dy < 25f64.to_radians().cos() {
                        continue;
                    }
                    if normals[j].0 * -dx + normals[j].1 * -dy < 0.5 {
                        continue;
                    }
                    let inside = inside_length(a, b, &edges, &edge_tree, &prep_union);

                    if inside > 1e-8 {
                        continue;
                    }
                    let mut owners = |p: C| {
                        owner_cache
                            .entry((p.0.to_bits(), p.1.to_bits()))
                            .or_insert_with(|| owners_at(p))
                            .clone()
                    };
                    let oa = owners(a);
                    let ob = owners(b);
                    let connector = Geom::Line(vec![a, b]);
                    let mut joined = false;
                    'outer: for &l in &oa {
                        for &r in &ob {
                            if l == r {
                                continue;
                            }
                            let joint = joint_cache.entry((l, r)).or_insert_with(|| {
                                sh::intersects(&local[l].geometry, &local[r].geometry).then(|| {
                                    sh::intersection(&local[l].geometry, &local[r].geometry)
                                })
                            });
                            if let Some(jg) = joint {
                                if sh::distance(&connector, jg) < minimum_mm {
                                    joined = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                    if joined {
                        continue;
                    }
                    let mut ids: Vec<String> = Vec::new();
                    let mut nets: Vec<String> = Vec::new();
                    for side in [&oa, &ob] {
                        for &k in side.iter() {
                            ids.push(local[k].identity.clone());
                            nets.push(local[k].net.clone());
                        }
                    }
                    ids.sort();
                    ids.dedup();
                    nets.sort();
                    nets.dedup();
                    let key = (ids.clone(), nets.clone());
                    if let Some(e) = findings.iter().find(|(k, _)| *k == key) {
                        if e.1.actual_value.unwrap_or(0.0) <= d {
                            continue;
                        }
                    }
                    let mut v = DRCViolation::new(
                        "physical_copper_gap",
                        "error",
                        format!(
                            "Physical copper gap {d:.4}mm < {minimum_mm:.4}mm; closest boundaries \
                             ({:.6}, {:.6}) and ({:.6}, {:.6})",
                            a.0, a.1, b.0, b.1
                        ),
                    )
                    .at((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
                    .layer(layer.clone())
                    .actual(d)
                    .required(minimum_mm)
                    .items(ids)
                    .nets(nets);
                    v.closest_locations = vec![a, b];
                    match findings.iter_mut().find(|(k, _)| *k == key) {
                        Some(e) => e.1 = v,
                        None => findings.push((key, v)),
                    }
                }
            }
        }
        findings.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, v) in findings {
            results.add(v);
        }
    }
    results
}
