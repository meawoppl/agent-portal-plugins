//! Copper geometry and bounded route search. Arc tessellation error <= 0.005 mm.
use crate::lint::{
    hash,
    intent::{Rect, Region, RoutePolicy},
    model::*,
};
use anyhow::{Context, Result};
use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, Intersects, LineString, MultiPolygon,
    Polygon, Validation,
};
use petgraph::{algo::astar, graph::NodeIndex, Graph, Undirected};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone)]
pub struct Copper {
    pub id: String,
    pub net: String,
    pub layers: Vec<String>,
    pub kind: String,
    pub at: Point,
    pub poly: MultiPolygon<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Text {
    pub id: String,
    pub text: String,
    pub layer: String,
    pub at: Point,
    pub bounds: Rect,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneInfo {
    pub id: String,
    pub net: String,
    pub layer: String,
    pub gap: f64,
    pub spoke: f64,
    pub connection: String,
    pub outlines: Vec<Vec<Point>>,
    pub filled: Vec<Vec<Point>>,
    pub fill_supported: bool,
    #[serde(skip)]
    pub copper: Vec<MultiPolygon<f64>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub net: String,
    pub layer: String,
    pub points: Vec<Point>,
    pub width: f64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Extra {
    pub arcs: Vec<Arc>,
    pub zones: Vec<ZoneInfo>,
    pub outlines: Vec<Vec<Point>>,
    pub keepouts: Vec<Region>,
    pub via_keepouts: Vec<Region>,
    pub texts: Vec<Text>,
    pub fields: BTreeMap<String, BTreeMap<String, String>>,
    pub pad_thermals: BTreeMap<String, String>,
    pub incomplete: Vec<String>,
    pub digest: String,
}
pub fn polygon(p: &[Point]) -> Polygon<f64> {
    let mut c: Vec<_> = p.iter().map(|p| (p.x, p.y)).collect();
    if c.first() != c.last() {
        if let Some(v) = c.first().copied() {
            c.push(v);
        }
    }
    Polygon::new(LineString::from(c), vec![])
}
/// KiCad links hole contours to the outer ring with zero-width return slits.
/// Even-odd overlay removes those doubled edges, preserving actual voids. Reject
/// repairs that materially change area (e.g. a genuinely crossing bow-tie).
pub fn filled_copper(points: &[Point]) -> Option<MultiPolygon<f64>> {
    if points.len() < 3 {
        return None;
    }
    let raw = MultiPolygon(vec![polygon(points)]);
    if raw.is_valid() {
        return Some(raw);
    }
    let fixed = raw.xor(&MultiPolygon(vec![]));
    let area = raw.unsigned_area();
    if !fixed.is_valid()
        || fixed.0.is_empty()
        || (fixed.unsigned_area() - area).abs() > (area * 1e-7).max(1e-6)
    {
        return None;
    }
    Some(fixed)
}
// Footprint rule-area coordinates are board coordinates in saved KiCad files.
fn read_keepout(s: &Sexp, out: &mut Extra) -> bool {
    let Some(k) = s.child("keepout") else {
        return false;
    };
    if k.get("tracks") != "not_allowed" && k.get("vias") != "not_allowed" {
        return true;
    }
    let layers: Vec<String> = s
        .child("layers")
        .map(|l| l.items().iter().skip(1).map(|n| n.atom().into()).collect())
        .unwrap_or_else(|| vec![s.get("layer").into()]);
    for p in s.children("polygon") {
        let pts = points(p);
        if pts.len() < 3
            || !polygon(&pts).is_valid()
            || p.child("pts")
                .is_some_and(|v| v.items().iter().skip(1).any(|q| q.tag() != "xy"))
        {
            out.incomplete
                .push(format!("unsupported keepout {}", id(s)));
            continue;
        }
        for layer in &layers {
            let region = Region {
                nets: vec![],
                id: id(s),
                layer: layer.clone(),
                polygon: pts.clone(),
            };
            if k.get("tracks") == "not_allowed" {
                out.keepouts.push(region.clone());
            }
            if k.get("vias") == "not_allowed" {
                out.via_keepouts.push(region);
            }
        }
    }
    true
}
pub fn rect(r: &Rect) -> Polygon<f64> {
    polygon(&[
        r.min,
        Point {
            x: r.max.x,
            y: r.min.y,
        },
        r.max,
        Point {
            x: r.min.x,
            y: r.max.y,
        },
    ])
}
pub fn bounds(p: &MultiPolygon<f64>) -> Option<Rect> {
    p.bounding_rect().map(|r| Rect {
        min: Point {
            x: r.min().x,
            y: r.min().y,
        },
        max: Point {
            x: r.max().x,
            y: r.max().y,
        },
    })
}
pub fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.min.x <= b.max.x && a.max.x >= b.min.x && a.min.y <= b.max.y && a.max.y >= b.min.y
}
pub fn line(p: &[Point], width: f64) -> MultiPolygon<f64> {
    LineString::from(p.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()).buffer(width / 2.)
}
pub fn disk(p: Point, r: f64) -> MultiPolygon<f64> {
    let n = 96;
    MultiPolygon(vec![polygon(
        &(0..n)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / n as f64;
                Point {
                    x: p.x + r * a.cos(),
                    y: p.y + r * a.sin(),
                }
            })
            .collect::<Vec<_>>(),
    )])
}
fn optional_num(s: &Sexp, key: &str, default: f64) -> f64 {
    s.child(key).and_then(|v| v.num(1).ok()).unwrap_or(default)
}
fn id(s: &Sexp) -> String {
    let v = if s.get("uuid").is_empty() {
        s.get("tstamp")
    } else {
        s.get("uuid")
    };
    if v.is_empty() {
        format!("fallback:{}", hash(&format!("{s:?}")))
    } else {
        v.into()
    }
}
fn points(s: &Sexp) -> Vec<Point> {
    s.child("pts")
        .map(|n| n.children("xy").filter_map(|p| point(p).ok()).collect())
        .unwrap_or_default()
}
fn arc_points(a: Point, m: Point, b: Point) -> Vec<Point> {
    let d = 2. * (a.x * (m.y - b.y) + m.x * (b.y - a.y) + b.x * (a.y - m.y));
    if d.abs() < 1e-12 {
        return vec![a, m, b];
    }
    let q = |p: Point| p.x * p.x + p.y * p.y;
    let cx = (q(a) * (m.y - b.y) + q(m) * (b.y - a.y) + q(b) * (a.y - m.y)) / d;
    let cy = (q(a) * (b.x - m.x) + q(m) * (a.x - b.x) + q(b) * (m.x - a.x)) / d;
    let center = Point { x: cx, y: cy };
    let r = a.distance(center);
    let angle = |p: Point| (p.y - cy).atan2(p.x - cx);
    let start = angle(a);
    let sweep = (angle(b) - start).rem_euclid(std::f64::consts::TAU);
    let mid = (angle(m) - start).rem_euclid(std::f64::consts::TAU);
    let sweep = if mid <= sweep {
        sweep
    } else {
        sweep - std::f64::consts::TAU
    };
    let step = (2. * (1. - 0.005 / r.max(0.005)).clamp(-1., 1.).acos()).max(0.001);
    let n = (sweep.abs() / step).ceil().max(2.) as usize;
    (0..=n.min(20000))
        .map(|i| {
            let t = start + sweep * i as f64 / n.min(20000) as f64;
            Point {
                x: cx + r * t.cos(),
                y: cy + r * t.sin(),
            }
        })
        .collect()
}
impl Extra {
    pub fn read(input: &str, b: &Board) -> Result<Self> {
        let root = parse(input)?;
        let mut out = Self::default();
        let nets: BTreeMap<_, _> = root.children("net").map(|x| (x.val(1), x.val(2))).collect();
        let net = |s: &Sexp| {
            let v = s.child("net");
            if !s.get("net_name").is_empty() {
                s.get("net_name").into()
            } else {
                v.map(|n| {
                    if n.items().len() > 2 {
                        n.val(2).to_owned()
                    } else {
                        nets.get(n.val(1)).copied().unwrap_or(n.val(1)).to_owned()
                    }
                })
                .unwrap_or_default()
            }
        };
        let mut edge_segments: Vec<(Point, Point)> = vec![];
        for s in root.items() {
            match s.tag() {
                "arc" => {
                    let get = |k: &str| point(s.child(k).context("arc missing coordinate")?);
                    let a = get("start")?;
                    let m = get("mid")?;
                    let z = get("end")?;
                    out.arcs.push(Arc {
                        id: id(s),
                        net: net(s),
                        layer: s.get("layer").into(),
                        points: arc_points(a, m, z),
                        width: optional_num(s, "width", 0.2),
                    });
                }
                "zone" => {
                    let ls: Vec<String> = s
                        .child("layers")
                        .map(|l| l.items().iter().skip(1).map(|n| n.atom().into()).collect())
                        .unwrap_or_else(|| vec![s.get("layer").into()]);
                    if read_keepout(s, &mut out) {
                        continue;
                    }
                    for layer in ls {
                        let fill = s.child("fill");
                        let gap = fill
                            .map(|f| optional_num(f, "thermal_gap", 0.5))
                            .unwrap_or(0.5);
                        let spoke = fill
                            .map(|f| optional_num(f, "thermal_bridge_width", 0.5))
                            .unwrap_or(0.5);
                        let filled: Vec<_> = s
                            .children("filled_polygon")
                            .filter(|p| p.get("layer") == layer || p.get("layer").is_empty())
                            .map(points)
                            .collect();
                        let outlines: Vec<_> = s.children("polygon").map(points).collect();
                        let copper: Vec<_> = filled.iter().map(|p| filled_copper(p)).collect();
                        let fill_supported = !(copper.iter().any(Option::is_none)
                            || s.children("filled_polygon").any(|p| {
                                p.child("pts").is_some_and(|pts| {
                                    pts.items().iter().skip(1).any(|v| v.tag() != "xy")
                                })
                            }));
                        if !fill_supported {
                            out.incomplete
                                .push(format!("unsupported or invalid filled polygon {}", id(s)));
                        }
                        if filled.is_empty() {
                            out.incomplete
                                .push(format!("unfilled zone {} on {layer}", id(s)));
                        }
                        out.zones.push(ZoneInfo {
                            id: id(s),
                            net: net(s),
                            layer,
                            gap,
                            spoke,
                            connection: s
                                .child("connect_pads")
                                .map(|c| {
                                    if c.val(1) == "yes" {
                                        "solid"
                                    } else if c.val(1) == "no" {
                                        "none"
                                    } else {
                                        "thermal"
                                    }
                                })
                                .unwrap_or("thermal")
                                .into(),
                            outlines,
                            filled,
                            fill_supported,
                            copper: copper
                                .into_iter()
                                .map(|p| p.unwrap_or_else(|| MultiPolygon(vec![])))
                                .collect(),
                        });
                    }
                }
                "gr_line" if s.get("layer") == "Edge.Cuts" => {
                    edge_segments.push((
                        point(s.child("start").context("edge start")?)?,
                        point(s.child("end").context("edge end")?)?,
                    ));
                }
                "gr_rect" if s.get("layer") == "Edge.Cuts" => {
                    let a = point(s.child("start").context("edge start")?)?;
                    let z = point(s.child("end").context("edge end")?)?;
                    out.outlines.push(vec![
                        a,
                        Point { x: z.x, y: a.y },
                        z,
                        Point { x: a.x, y: z.y },
                    ]);
                }
                "gr_arc" if s.get("layer") == "Edge.Cuts" => {
                    let pts = arc_points(
                        point(s.child("start").context("edge arc start")?)?,
                        point(s.child("mid").context("edge arc mid")?)?,
                        point(s.child("end").context("edge arc end")?)?,
                    );
                    for p in pts.windows(2) {
                        edge_segments.push((p[0], p[1]));
                    }
                }
                "gr_circle" if s.get("layer") == "Edge.Cuts" => {
                    let a = point(s.child("center").context("circle center")?)?;
                    let z = point(s.child("end").context("circle end")?)?;
                    out.outlines.push(
                        disk(a, a.distance(z)).0[0]
                            .exterior()
                            .0
                            .iter()
                            .map(|p| Point { x: p.x, y: p.y })
                            .collect(),
                    );
                }
                "gr_text" => {
                    if let Some(t) = text(s, None) {
                        out.texts.push(t)
                    }
                }
                "footprint" | "module" => {
                    let fid = id(s);
                    if let Some(f) = b.parts.iter().find(|p| p.id == fid) {
                        let fields = s
                            .children("property")
                            .map(|p| (p.val(1).into(), p.val(2).into()))
                            .collect();
                        out.fields.insert(f.reference.clone(), fields);
                        for p in s.children("pad") {
                            if let Some(z) = p.child("zone_connect") {
                                out.pad_thermals.insert(
                                    id(p),
                                    match z.val(1) {
                                        "0" => "none",
                                        "2" => "solid",
                                        _ => "thermal",
                                    }
                                    .into(),
                                );
                            }
                        }
                        for t in s
                            .items()
                            .iter()
                            .filter(|s| s.tag() == "fp_text" || s.tag() == "property")
                        {
                            if !t.items().iter().any(|x| {
                                x.atom() == "hide" || x.tag() == "hide" && x.val(1) == "yes"
                            }) {
                                if let Some(t) = text(t, Some(f)) {
                                    out.texts.push(t)
                                }
                            }
                        }
                        for z in s.children("zone") {
                            if !read_keepout(z, &mut out) {
                                out.incomplete
                                    .push("footprint zone geometry not expanded".into());
                            }
                        }
                    }
                }
                _ if s.get("layer") == "Edge.Cuts" => {
                    out.incomplete
                        .push(format!("unsupported Edge.Cuts item {}", s.tag()));
                }
                _ => {}
            }
        }
        while let Some((a, z)) = edge_segments.pop() {
            let mut poly = vec![a, z];
            while poly.last().unwrap().distance(a) > 0.001 {
                let p = *poly.last().unwrap();
                if let Some(i) = edge_segments
                    .iter()
                    .position(|(x, y)| p.distance(*x) < 0.001 || p.distance(*y) < 0.001)
                {
                    let (x, y) = edge_segments.swap_remove(i);
                    poly.push(if p.distance(x) < 0.001 { y } else { x });
                } else {
                    out.incomplete.push("open Edge.Cuts chain".into());
                    break;
                }
            }
            if poly.len() >= 4 && poly.last().unwrap().distance(a) <= 0.001 {
                out.outlines.push(poly);
            }
        }
        for (s, n) in &b.unsupported {
            if s.starts_with("pad shape") || s.contains("graphics") {
                out.incomplete.push(format!("{n} unsupported {s}"));
            }
        }
        out.texts.sort_by(|a, b| a.id.cmp(&b.id));
        out.arcs.sort_by(|a, b| a.id.cmp(&b.id));
        out.zones
            .sort_by(|a, b| (&a.id, &a.layer).cmp(&(&b.id, &b.layer)));
        out.digest = hash(&serde_json::to_string(&out)?);
        Ok(out)
    }
}
fn text(s: &Sexp, f: Option<&Part>) -> Option<Text> {
    let layer = s.get("layer");
    if !layer.ends_with("SilkS") {
        return None;
    }
    let at = s.child("at").and_then(|x| point(x).ok())?;
    let angle = s.child("at").and_then(|s| s.num(3).ok()).unwrap_or(0.);
    let at = f.map_or(at, |f| at.rotate(f.angle).plus(f.at));
    let content = s.val(if s.tag() == "gr_text" { 1 } else { 2 }).to_owned();
    let size = s
        .child("effects")
        .and_then(|x| x.child("font"))
        .and_then(|x| x.child("size"))
        .and_then(|x| point(x).ok())
        .unwrap_or(Point { x: 1., y: 1. });
    let width = content.chars().count() as f64 * size.x * 0.8;
    let h = size.y;
    let corners = [
        Point {
            x: -width / 2.,
            y: -h / 2.,
        },
        Point {
            x: width / 2.,
            y: -h / 2.,
        },
        Point {
            x: width / 2.,
            y: h / 2.,
        },
        Point {
            x: -width / 2.,
            y: h / 2.,
        },
    ]
    .map(|p| p.rotate(angle).plus(at));
    let min = Point {
        x: corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
        y: corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
    };
    let max = Point {
        x: corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max),
        y: corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max),
    };
    Some(Text {
        id: id(s),
        text: content,
        layer: layer.into(),
        at,
        bounds: Rect { min, max },
    })
}
pub fn pad_poly(p: &Pad) -> MultiPolygon<f64> {
    let outer = pad_outer(p);
    if let Some(d) = p.drill {
        let r = d.x.min(d.y) / 2.;
        let hole = line(
            &[
                Point {
                    x: -(d.x / 2. - r),
                    y: -(d.y / 2. - r),
                }
                .rotate(p.angle)
                .plus(p.at),
                Point {
                    x: d.x / 2. - r,
                    y: d.y / 2. - r,
                }
                .rotate(p.angle)
                .plus(p.at),
            ],
            r * 2.,
        );
        outer.difference(&hole)
    } else {
        outer
    }
}
fn pad_outer(p: &Pad) -> MultiPolygon<f64> {
    let hx = p.size.x / 2.;
    let hy = p.size.y / 2.;
    if p.shape == "circle" {
        return disk(p.at, hx);
    }
    if p.shape == "oval" {
        let r = hx.min(hy);
        return line(
            &[
                Point {
                    x: -(hx - r),
                    y: -(hy - r),
                }
                .rotate(p.angle)
                .plus(p.at),
                Point {
                    x: hx - r,
                    y: hy - r,
                }
                .rotate(p.angle)
                .plus(p.at),
            ],
            r * 2.,
        );
    }
    if p.shape == "roundrect" {
        let r = hx.min(hy) * 2. * p.round_ratio.clamp(0., 0.5);
        if r > 0. {
            let core = [
                Point {
                    x: -hx + r,
                    y: -hy + r,
                },
                Point {
                    x: hx - r,
                    y: -hy + r,
                },
                Point {
                    x: hx - r,
                    y: hy - r,
                },
                Point {
                    x: -hx + r,
                    y: hy - r,
                },
            ]
            .map(|q| q.rotate(p.angle).plus(p.at));
            return polygon(&core).buffer(r);
        }
    }
    let corners = [
        Point { x: -hx, y: -hy },
        Point { x: hx, y: -hy },
        Point { x: hx, y: hy },
        Point { x: -hx, y: hy },
    ]
    .map(|q| q.rotate(p.angle).plus(p.at));
    MultiPolygon(vec![polygon(&corners)])
}
pub struct Geometry {
    pub items: Vec<Copper>,
    pub graph: Graph<usize, (), Undirected>,
    pub outline: Option<MultiPolygon<f64>>,
    pub extra: Extra,
    pub uncertain: bool,
}
impl Geometry {
    pub fn new(b: &Board, extra: Extra, policy: &RoutePolicy) -> Self {
        let mut items = vec![];
        for t in &b.tracks {
            items.push(Copper {
                id: t.id.clone(),
                net: t.net.clone(),
                layers: vec![t.layer.clone()],
                kind: "track".into(),
                at: t.a,
                poly: line(&[t.a, t.b], t.width),
            });
        }
        for a in &extra.arcs {
            items.push(Copper {
                id: a.id.clone(),
                net: a.net.clone(),
                layers: vec![a.layer.clone()],
                kind: "arc".into(),
                at: a.points[0],
                poly: line(&a.points, a.width),
            });
        }
        for v in &b.vias {
            items.push(Copper {
                id: v.id.clone(),
                net: v.net.clone(),
                layers: b.via_layers(v),
                kind: "via".into(),
                at: v.at,
                poly: disk(v.at, v.size / 2.).difference(&disk(v.at, v.drill / 2.)),
            });
        }
        for p in &b.pads {
            if p.kind != "np_thru_hole" {
                items.push(Copper {
                    id: p.id.clone(),
                    net: p.net.clone(),
                    layers: b
                        .copper_layers
                        .iter()
                        .filter(|l| on_layer(&p.layers, l))
                        .cloned()
                        .collect(),
                    kind: "pad".into(),
                    at: p.at,
                    poly: pad_poly(p),
                });
            }
        }
        for p in b.pads.iter().filter(|p| p.kind == "np_thru_hole") {
            items.push(Copper {
                id: p.id.clone(),
                net: String::new(),
                layers: b.copper_layers.clone(),
                kind: "hole".into(),
                at: p.at,
                poly: pad_outer(p),
            });
        }
        for z in &extra.zones {
            for (i, (points, poly)) in z.filled.iter().zip(&z.copper).enumerate() {
                if z.fill_supported && !poly.0.is_empty() {
                    items.push(Copper {
                        id: format!("{}:fill:{i}:{}", z.id, z.layer),
                        net: z.net.clone(),
                        layers: vec![z.layer.clone()],
                        kind: "zone".into(),
                        at: points[0],
                        poly: poly.clone(),
                    });
                }
            }
        }
        let mut graph = Graph::new_undirected();
        for i in 0..items.len() {
            graph.add_node(i);
        }
        let bounds: Vec<_> = items.iter().map(|p| bounds(&p.poly)).collect();
        for i in 0..items.len() {
            for j in i + 1..items.len() {
                let a = &items[i];
                let z = &items[j];
                if a.net.is_empty()
                    || a.net != z.net
                    || !a.layers.iter().any(|l| z.layers.contains(l))
                {
                    continue;
                }
                if matches!((&bounds[i], &bounds[j]), (Some(a), Some(z)) if !overlaps(a, z)) {
                    continue;
                }
                if a.poly.intersects(&z.poly) {
                    graph.add_edge(NodeIndex::new(i), NodeIndex::new(j), ());
                }
            }
        }
        use geo::BooleanOps;
        let mut outline = None;
        if policy.outline.len() >= 3 {
            outline = Some(MultiPolygon(vec![polygon(&policy.outline)]));
        } else {
            for p in &extra.outlines {
                if p.len() >= 3 {
                    let poly = MultiPolygon(vec![polygon(p)]);
                    outline = Some(
                        outline.map_or_else(|| poly.clone(), |x: MultiPolygon<f64>| x.xor(&poly)),
                    );
                }
            }
        }
        let uncertain = !extra.incomplete.is_empty();
        Self {
            items,
            graph,
            outline,
            extra,
            uncertain,
        }
    }
    pub fn components(&self, skip: Option<&str>) -> Vec<Vec<String>> {
        let mut seen = BTreeSet::new();
        let mut groups = vec![];
        for i in 0..self.items.len() {
            if seen.contains(&i) || skip == Some(&self.items[i].id) {
                continue;
            }
            let mut stack = vec![i];
            let mut group = vec![];
            while let Some(j) = stack.pop() {
                if !seen.insert(j) {
                    continue;
                }
                if self.items[j].kind == "pad" {
                    group.push(self.items[j].id.clone())
                }
                for k in self.graph.neighbors(NodeIndex::new(j)) {
                    if skip != Some(&self.items[k.index()].id) {
                        stack.push(k.index());
                    }
                }
            }
            if !group.is_empty() {
                group.sort();
                groups.push(group)
            }
        }
        groups.sort();
        groups
    }
    pub fn islands(&self) -> Vec<Vec<usize>> {
        let mut seen = BTreeSet::new();
        let mut groups = vec![];
        for i in 0..self.items.len() {
            if seen.contains(&i) {
                continue;
            }
            let mut stack = vec![i];
            let mut group = vec![];
            while let Some(j) = stack.pop() {
                if !seen.insert(j) {
                    continue;
                }
                group.push(j);
                for k in self.graph.neighbors(NodeIndex::new(j)) {
                    stack.push(k.index())
                }
            }
            groups.push(group)
        }
        groups
    }
    pub fn legal(
        &self,
        path: &[Point],
        width: f64,
        net: &str,
        layer: &str,
        policy: &RoutePolicy,
    ) -> bool {
        let Some(outline) = &self.outline else {
            return false;
        };
        if !outline.contains(&line(path, width + 2. * (policy.edge_clearance_mm + 0.005))) {
            return false;
        }
        self.clearance_free(path, width, net, layer, policy)
    }
    /// Copper-envelope test independent of board outline availability.
    pub fn clearance_free(
        &self,
        path: &[Point],
        width: f64,
        net: &str,
        layer: &str,
        policy: &RoutePolicy,
    ) -> bool {
        let copper = line(path, width + 2. * (policy.clearance_mm + 0.005));
        if policy.keepouts.iter().chain(&self.extra.keepouts).any(|r| {
            (r.layer == layer || r.layer == "*.Cu") && polygon(&r.polygon).intersects(&copper)
        }) {
            return false;
        }
        !self
            .items
            .iter()
            .filter(|x| x.net != net && x.layers.iter().any(|l| l == layer))
            .any(|x| x.poly.intersects(&copper))
    }
    /// Finite 45-degree visibility graph over endpoints, octilinear elbows and obstacle envelope corners.
    pub fn shortcut(
        &self,
        a: Point,
        z: Point,
        width: f64,
        net: &str,
        layer: &str,
        p: &RoutePolicy,
    ) -> (Option<Vec<Point>>, bool) {
        if self.uncertain || self.outline.is_none() {
            return (None, false);
        }
        let mut points = vec![a, z];
        let mut anchors = vec![a, z];
        let min = Point {
            x: a.x.min(z.x) - p.search_margin_mm,
            y: a.y.min(z.y) - p.search_margin_mm,
        };
        let max = Point {
            x: a.x.max(z.x) + p.search_margin_mm,
            y: a.y.max(z.y) + p.search_margin_mm,
        };
        for item in self
            .items
            .iter()
            .filter(|o| o.net != net && o.layers.contains(&layer.to_owned()))
        {
            if let Some(r) = bounds(&item.poly) {
                let d = width / 2. + p.clearance_mm + p.grid_mm;
                for v in [
                    Point {
                        x: r.min.x - d,
                        y: r.min.y - d,
                    },
                    Point {
                        x: r.min.x - d,
                        y: r.max.y + d,
                    },
                    Point {
                        x: r.max.x + d,
                        y: r.min.y - d,
                    },
                    Point {
                        x: r.max.x + d,
                        y: r.max.y + d,
                    },
                ] {
                    if v.x >= min.x && v.y >= min.y && v.x <= max.x && v.y <= max.y {
                        anchors.push(v)
                    }
                }
            }
        }
        // Cap is on generated graph vertices. Exhaustion is visible, never interpreted as no route.
        let limit = p.max_nodes.min(1000);
        if anchors.len() * anchors.len() * 4 > limit {
            return (None, true);
        }
        for (i, x) in anchors.iter().enumerate() {
            for y in &anchors[i + 1..] {
                points.push(Point { x: x.x, y: y.y });
                points.push(Point { x: y.x, y: x.y });
                let dx = y.x - x.x;
                let dy = y.y - x.y;
                let m = dx.abs().min(dy.abs());
                points.push(Point {
                    x: x.x + dx.signum() * m,
                    y: x.y + dy.signum() * m,
                });
                points.push(Point {
                    x: y.x - dx.signum() * m,
                    y: y.y - dy.signum() * m,
                });
            }
        }
        points.extend(anchors.into_iter().skip(2));
        let mut graph = Graph::<Point, f64, Undirected>::new_undirected();
        for v in &points {
            graph.add_node(*v);
        }
        for i in 0..points.len() {
            for j in i + 1..points.len() {
                let d = points[i].minus(points[j]);
                let angle_ok =
                    d.x.abs() < 1e-6 || d.y.abs() < 1e-6 || (d.x.abs() - d.y.abs()).abs() < 1e-6;
                if angle_ok && self.legal(&[points[i], points[j]], width, net, layer, p) {
                    graph.add_edge(
                        NodeIndex::new(i),
                        NodeIndex::new(j),
                        points[i].distance(points[j]),
                    );
                }
            }
        }
        let result = astar(
            &graph,
            NodeIndex::new(0),
            |n| n.index() == 1,
            |e| *e.weight(),
            |n| graph[n].distance(z),
        )
        .map(|(_, nodes)| nodes.into_iter().map(|n| graph[n]).collect());
        (result, false)
    }
}
pub fn route_length(b: &Board, x: &Extra, net: &str) -> f64 {
    b.tracks
        .iter()
        .filter(|t| t.net == net)
        .map(|t| t.a.distance(t.b))
        .sum::<f64>()
        + x.arcs
            .iter()
            .filter(|t| t.net == net)
            .map(|a| {
                a.points
                    .windows(2)
                    .map(|p| p[0].distance(p[1]))
                    .sum::<f64>()
            })
            .sum::<f64>()
}
