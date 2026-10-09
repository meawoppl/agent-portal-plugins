use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}
impl Sexp {
    pub fn atom(&self) -> &str {
        if let Self::Atom(s) = self {
            s
        } else {
            ""
        }
    }
    pub fn items(&self) -> &[Self] {
        if let Self::List(v) = self {
            v
        } else {
            &[]
        }
    }
    pub fn tag(&self) -> &str {
        self.items().first().map_or("", Self::atom)
    }
    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.items().iter().filter(move |s| s.tag() == name)
    }
    pub fn child(&self, name: &str) -> Option<&Self> {
        self.items().iter().find(|s| s.tag() == name)
    }
    pub fn val(&self, i: usize) -> &str {
        self.items().get(i).map_or("", Self::atom)
    }
    pub fn get(&self, key: &str) -> &str {
        self.child(key).map_or("", |s| s.val(1))
    }
    pub fn num(&self, i: usize) -> Result<f64> {
        let n: f64 = self
            .val(i)
            .parse()
            .with_context(|| format!("expected number in {} at {i}", self.tag()))?;
        if !n.is_finite() {
            bail!("non-finite coordinate")
        }
        Ok(n)
    }
}
pub fn parse(input: &str) -> Result<Sexp> {
    fn node(c: &[char], p: &mut usize, depth: usize) -> Result<Sexp> {
        if depth > 128 {
            bail!("S-expression nesting exceeds 128")
        }
        while *p < c.len() && c[*p].is_whitespace() {
            *p += 1;
        }
        let ch = *c.get(*p).context("unexpected EOF")?;
        *p += 1;
        if ch == '(' {
            let mut v = Vec::new();
            loop {
                while *p < c.len() && c[*p].is_whitespace() {
                    *p += 1;
                }
                if c.get(*p) == Some(&')') {
                    *p += 1;
                    break;
                }
                v.push(node(c, p, depth + 1)?);
            }
            Ok(Sexp::List(v))
        } else if ch == '"' {
            let mut s = String::new();
            loop {
                let ch = *c.get(*p).context("unterminated string")?;
                *p += 1;
                if ch == '"' {
                    break;
                }
                if ch == '\\' {
                    let e = *c.get(*p).context("unterminated escape")?;
                    *p += 1;
                    s.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        _ => e,
                    });
                } else {
                    s.push(ch);
                }
            }
            Ok(Sexp::Atom(s))
        } else if ch == ')' {
            bail!("unexpected closing parenthesis")
        } else {
            let mut s = String::from(ch);
            while *p < c.len() && !c[*p].is_whitespace() && c[*p] != '(' && c[*p] != ')' {
                s.push(c[*p]);
                *p += 1;
            }
            Ok(Sexp::Atom(s))
        }
    }
    let c: Vec<_> = input.chars().collect();
    let mut p = 0;
    let result = node(&c, &mut p, 0)?;
    if c[p..].iter().any(|c| !c.is_whitespace()) {
        bail!("trailing data")
    }
    Ok(result)
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    pub fn distance(self, b: Self) -> f64 {
        (self.x - b.x).hypot(self.y - b.y)
    }
    pub fn rotate(self, angle: f64) -> Self {
        let (s, c) = angle.to_radians().sin_cos();
        Self {
            x: self.x * c + self.y * s,
            y: -self.x * s + self.y * c,
        }
    }
    pub fn plus(self, b: Self) -> Self {
        Self {
            x: self.x + b.x,
            y: self.y + b.y,
        }
    }
    pub fn minus(self, b: Self) -> Self {
        Self {
            x: self.x - b.x,
            y: self.y - b.y,
        }
    }
}
pub fn point(s: &Sexp) -> Result<Point> {
    Ok(Point {
        x: s.num(1)?,
        y: s.num(2)?,
    })
}
#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub id: String,
    pub stable: bool,
    pub net: String,
    pub layer: String,
    pub a: Point,
    pub b: Point,
    pub width: f64,
}
/// Circular copper track (`arc`), modelled so joins land on it and its own
/// ends are checked; clearance rules still treat it as unmodelled geometry.
#[derive(Debug, Clone, Serialize)]
pub struct ArcTrack {
    pub id: String,
    pub stable: bool,
    pub net: String,
    pub layer: String,
    pub start: Point,
    pub mid: Point,
    pub end: Point,
    pub width: f64,
}
impl ArcTrack {
    /// Centre and radius of the circle through the three points; `None`
    /// when they are collinear (a degenerate arc is treated as its chord).
    pub fn circle(&self) -> Option<(Point, f64)> {
        let (a, b, c) = (self.start, self.mid, self.end);
        let d = 2. * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
        if d.abs() < 1e-12 {
            return None;
        }
        let (a2, b2, c2) = (
            a.x * a.x + a.y * a.y,
            b.x * b.x + b.y * b.y,
            c.x * c.x + c.y * c.y,
        );
        let centre = Point {
            x: (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d,
            y: (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d,
        };
        Some((centre, centre.distance(a)))
    }
    /// Distance from `p` to the arc's centreline (the sweep from `start`
    /// through `mid` to `end`); beyond the sweep, to the nearer end.
    pub fn distance(&self, p: Point) -> f64 {
        let Some((c, r)) = self.circle() else {
            return line_distance(p, self.start, self.end);
        };
        let bearing = |q: Point| (q.y - c.y).atan2(q.x - c.x);
        let sweep = |from: f64, to: f64| (to - from).rem_euclid(std::f64::consts::TAU);
        let (a0, am, a1, ap) = (
            bearing(self.start),
            bearing(self.mid),
            bearing(self.end),
            bearing(p),
        );
        // The arc runs counter-clockwise when the mid point comes before the
        // end on the counter-clockwise sweep from the start.
        let within = if sweep(a0, am) <= sweep(a0, a1) {
            sweep(a0, ap) <= sweep(a0, a1)
        } else {
            sweep(a1, ap) <= sweep(a1, a0)
        };
        if within {
            (p.distance(c) - r).abs()
        } else {
            p.distance(self.start).min(p.distance(self.end))
        }
    }
    /// Arc length in mm (chord length when degenerate).
    pub fn length(&self) -> f64 {
        let Some((c, r)) = self.circle() else {
            return self.start.distance(self.end);
        };
        let bearing = |q: Point| (q.y - c.y).atan2(q.x - c.x);
        let sweep = |from: f64, to: f64| (to - from).rem_euclid(std::f64::consts::TAU);
        let (a0, am, a1) = (bearing(self.start), bearing(self.mid), bearing(self.end));
        let angle = if sweep(a0, am) <= sweep(a0, a1) {
            sweep(a0, a1)
        } else {
            sweep(a1, a0)
        };
        r * angle
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Via {
    pub id: String,
    pub stable: bool,
    pub net: String,
    pub at: Point,
    pub size: f64,
    pub drill: f64,
    pub layers: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Pad {
    pub geometry_hash: String,
    pub id: String,
    pub stable: bool,
    pub reference: String,
    pub number: String,
    pub net: String,
    pub at: Point,
    pub size: Point,
    pub angle: f64,
    pub shape: String,
    pub round_ratio: f64,
    pub drill: Option<Point>,
    pub kind: String,
    pub layers: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub id: String,
    pub stable: bool,
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub at: Point,
    pub angle: f64,
    pub layer: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    pub net: String,
    pub layers: Vec<String>,
    pub geometry_hash: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub tracks: Vec<Track>,
    pub arcs: Vec<ArcTrack>,
    pub vias: Vec<Via>,
    pub pads: Vec<Pad>,
    pub parts: Vec<Part>,
    pub zones: Vec<Zone>,
    pub copper_layers: Vec<String>,
    pub unsupported: BTreeMap<String, usize>,
    pub unmodeled_geometry: Vec<String>,
    /// Located copy of `unmodeled_geometry` for local evidence scoping.
    #[serde(skip)]
    pub unmodeled_located: Vec<Unmodeled>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Unmodeled {
    pub hash: String,
    /// Footprint origin or first coordinate; `None` when no position could be read.
    pub at: Option<Point>,
    /// Owning footprint id, empty for board-level items.
    pub owner: String,
}
/// First readable coordinate of a board graphic.
fn anchor(s: &Sexp) -> Option<Point> {
    for k in ["start", "center", "at"] {
        if let Some(p) = s.child(k).and_then(|c| point(c).ok()) {
            return Some(p);
        }
    }
    s.child("pts")
        .and_then(|p| p.child("xy"))
        .and_then(|c| point(c).ok())
}
fn identity(s: &Sexp, parent: &str) -> (String, bool) {
    let id = if s.get("uuid").is_empty() {
        s.get("tstamp")
    } else {
        s.get("uuid")
    };
    if !id.is_empty() {
        (id.to_owned(), true)
    } else {
        (
            format!("fallback:{}", crate::lint::hash(&format!("{parent}:{s:?}"))),
            false,
        )
    }
}
fn layers(s: &Sexp) -> Vec<String> {
    if let Some(v) = s.child("layers") {
        v.items()
            .iter()
            .skip(1)
            .map(|x| x.atom().to_owned())
            .collect()
    } else {
        vec![s.get("layer").to_owned()]
    }
}
fn net(s: &Sexp, nets: &BTreeMap<String, String>) -> String {
    let Some(n) = s.child("net") else {
        return String::new();
    };
    if n.items().len() > 2 {
        n.val(2).to_owned()
    } else {
        nets.get(n.val(1)).cloned().unwrap_or_else(|| {
            if n.val(1) == "0" {
                String::new()
            } else {
                n.val(1).to_owned()
            }
        })
    }
}
fn angle(s: &Sexp) -> Result<f64> {
    if s.val(3).is_empty() {
        Ok(0.)
    } else {
        s.num(3)
    }
}
fn required_point(s: &Sexp, k: &str) -> Result<Point> {
    point(
        s.child(k)
            .with_context(|| format!("{} missing {k}", s.tag()))?,
    )
}
impl Board {
    fn unmodeled(&mut self, hash: String, at: Option<Point>, owner: &str) {
        self.unmodeled_geometry.push(hash.clone());
        self.unmodeled_located.push(Unmodeled {
            hash,
            at,
            owner: owner.into(),
        });
    }
    pub fn read(input: &str) -> Result<Self> {
        let root = parse(input)?;
        if root.tag() != "kicad_pcb" {
            bail!("expected kicad_pcb document")
        }
        let nets = root
            .children("net")
            .map(|s| (s.val(1).to_owned(), s.val(2).to_owned()))
            .collect();
        let copper_layers = root
            .child("layers")
            .context("missing layers")?
            .items()
            .iter()
            .skip(1)
            .filter_map(|s| {
                let l = s.val(1);
                l.ends_with(".Cu").then(|| l.to_owned())
            })
            .collect();
        let mut b = Self {
            tracks: vec![],
            arcs: vec![],
            vias: vec![],
            pads: vec![],
            parts: vec![],
            zones: vec![],
            copper_layers,
            unsupported: BTreeMap::new(),
            unmodeled_geometry: vec![],
            unmodeled_located: vec![],
        };
        for s in root.items() {
            let (id, stable) = identity(s, "");
            match s.tag() {
                "segment" => {
                    let width = s.child("width").context("segment width missing")?.num(1)?;
                    if width <= 0. {
                        bail!("nonpositive trace width")
                    };
                    b.tracks.push(Track {
                        id,
                        stable,
                        net: net(s, &nets),
                        layer: s.get("layer").into(),
                        a: required_point(s, "start")?,
                        b: required_point(s, "end")?,
                        width,
                    });
                }
                "via" => {
                    let size = s.child("size").context("via size missing")?.num(1)?;
                    let drill = s.child("drill").context("via drill missing")?.num(1)?;
                    if size <= 0. || drill <= 0. {
                        bail!("nonpositive via size/drill")
                    };
                    b.vias.push(Via {
                        id,
                        stable,
                        net: net(s, &nets),
                        at: required_point(s, "at")?,
                        size,
                        drill,
                        layers: layers(s),
                    });
                }
                "footprint" | "module" => {
                    let at = required_point(s, "at")?;
                    let a = angle(s.child("at").unwrap())?;
                    let property = |key: &str| {
                        s.children("property")
                            .find(|p| p.val(1) == key)
                            .map(|p| p.val(2))
                            .or_else(|| {
                                s.children("fp_text")
                                    .find(|p| p.val(1) == key.to_lowercase())
                                    .map(|p| p.val(2))
                            })
                            .unwrap_or("")
                            .to_owned()
                    };
                    for g in s
                        .items()
                        .iter()
                        .filter(|g| g.get("layer").ends_with(".Cu") && g.tag().starts_with("fp_"))
                    {
                        b.unmodeled(
                            crate::lint::hash(&format!("{g:?}:{at:?}:{a}")),
                            Some(at),
                            &id,
                        );
                        *b.unsupported
                            .entry("footprint copper graphics".into())
                            .or_default() += 1;
                    }
                    for z in s.children("zone") {
                        b.unmodeled(
                            crate::lint::hash(&format!("{z:?}:{at:?}:{a}")),
                            Some(at),
                            &id,
                        );
                        *b.unsupported.entry("footprint zones".into()).or_default() += 1;
                    }
                    let reference = property("Reference");
                    b.parts.push(Part {
                        id: id.clone(),
                        stable,
                        reference: reference.clone(),
                        value: property("Value"),
                        footprint: s.val(1).into(),
                        at,
                        angle: a,
                        layer: s.get("layer").into(),
                    });
                    for p in s.children("pad") {
                        let (pid, ps) = identity(p, &id);
                        let pa = p.child("at").context("pad at missing")?;
                        let local = point(pa)?;
                        // Footprint transforms local position. KiCad stores pad orientation in board axes.
                        let size = required_point(p, "size")?;
                        if size.x <= 0. || size.y <= 0. {
                            bail!("nonpositive pad size")
                        }
                        if !["circle", "rect", "oval", "roundrect"].contains(&p.val(3)) {
                            *b.unsupported
                                .entry(format!("pad shape {}", p.val(3)))
                                .or_default() += 1;
                        }
                        b.pads.push(Pad {
                            geometry_hash: crate::lint::hash(&format!("{p:?}")),
                            id: pid,
                            stable: ps,
                            reference: reference.clone(),
                            number: p.val(1).into(),
                            kind: p.val(2).into(),
                            shape: p.val(3).into(),
                            round_ratio: p
                                .child("roundrect_rratio")
                                .map(|r| r.num(1))
                                .transpose()?
                                .unwrap_or(0.25),
                            drill: p
                                .child("drill")
                                .map(|d| {
                                    if d.val(1) == "oval" {
                                        Ok(Point {
                                            x: d.num(2)?,
                                            y: d.num(3)?,
                                        })
                                    } else {
                                        let x = d.num(1)?;
                                        Ok::<_, anyhow::Error>(Point { x, y: x })
                                    }
                                })
                                .transpose()?,
                            net: net(p, &nets),
                            at: local.rotate(a).plus(at),
                            size,
                            angle: angle(pa)?,
                            layers: layers(p),
                        });
                    }
                }
                "zone" => {
                    let n = if s.get("net_name").is_empty() {
                        net(s, &nets)
                    } else {
                        s.get("net_name").to_owned()
                    };
                    b.zones.push(Zone {
                        net: n,
                        layers: layers(s),
                        geometry_hash: crate::lint::hash(&format!("{s:?}")),
                    });
                }
                "arc" => {
                    let width = s.child("width").context("arc width missing")?.num(1)?;
                    if width <= 0. {
                        bail!("nonpositive arc width")
                    };
                    b.arcs.push(ArcTrack {
                        id,
                        stable,
                        net: net(s, &nets),
                        layer: s.get("layer").into(),
                        start: required_point(s, "start")?,
                        mid: required_point(s, "mid")?,
                        end: required_point(s, "end")?,
                        width,
                    });
                    // Joins and open ends see the arc; clearance rules do not.
                    b.unmodeled(crate::lint::hash(&format!("{s:?}")), anchor(s), "");
                    *b.unsupported
                        .entry("copper arcs (joins only; clearance not analyzed)".into())
                        .or_default() += 1;
                }
                tag if tag.starts_with("gr_") && s.get("layer").ends_with(".Cu") => {
                    b.unmodeled(crate::lint::hash(&format!("{s:?}")), anchor(s), "");
                    *b.unsupported
                        .entry("board copper graphics".into())
                        .or_default() += 1;
                }
                _ => {}
            }
        }
        let mut ids = BTreeSet::new();
        for (id, stable) in b
            .tracks
            .iter()
            .map(|x| (&x.id, x.stable))
            .chain(b.arcs.iter().map(|x| (&x.id, x.stable)))
            .chain(b.vias.iter().map(|x| (&x.id, x.stable)))
            .chain(b.pads.iter().map(|x| (&x.id, x.stable)))
            .chain(b.parts.iter().map(|x| (&x.id, x.stable)))
        {
            if stable && !ids.insert(id) {
                bail!("duplicate UUID/tstamp {id}")
            }
        }
        for t in &mut b.tracks {
            if (t.a.x, t.a.y) > (t.b.x, t.b.y) {
                std::mem::swap(&mut t.a, &mut t.b)
            }
        }
        b.tracks.sort_by(|a, b| a.id.cmp(&b.id));
        b.arcs.sort_by(|a, b| a.id.cmp(&b.id));
        b.vias.sort_by(|a, b| a.id.cmp(&b.id));
        b.pads.sort_by(|a, b| a.id.cmp(&b.id));
        b.parts.sort_by(|a, b| a.id.cmp(&b.id));
        b.unmodeled_geometry.sort();
        if b.copper_layers.is_empty() {
            bail!("no copper layers")
        }
        Ok(b)
    }
    pub fn via_layers(&self, v: &Via) -> Vec<String> {
        if v.layers.len() == 2 {
            let a = self.copper_layers.iter().position(|x| x == &v.layers[0]);
            let z = self.copper_layers.iter().position(|x| x == &v.layers[1]);
            if let (Some(a), Some(z)) = (a, z) {
                return self.copper_layers[a.min(z)..=a.max(z)].to_vec();
            }
        }
        v.layers.clone()
    }
}
pub fn on_layer(layers: &[String], layer: &str) -> bool {
    layers.iter().any(|l| {
        l == layer || l == "*.Cu" || (l == "F&B.Cu" && (layer == "F.Cu" || layer == "B.Cu"))
    })
}
pub fn line_distance(p: Point, a: Point, b: Point) -> f64 {
    let d = b.minus(a);
    let l = d.x * d.x + d.y * d.y;
    if l == 0. {
        return p.distance(a);
    }
    let t = ((p.x - a.x) * d.x + (p.y - a.y) * d.y) / l;
    p.distance(Point {
        x: a.x + t.clamp(0., 1.) * d.x,
        y: a.y + t.clamp(0., 1.) * d.y,
    })
}
/// Distance to a conservative pad copper envelope; custom pads use bounding rectangle.
pub fn pad_distance(p: Point, pad: &Pad) -> f64 {
    let q = p.minus(pad.at).rotate(-pad.angle);
    let hx = pad.size.x / 2.;
    let hy = pad.size.y / 2.;
    if pad.shape == "circle" {
        return (q.x.hypot(q.y) - hx.max(hy)).max(0.);
    }
    if pad.shape == "oval" {
        let r = hx.min(hy);
        return (line_distance(
            q,
            Point {
                x: -(hx - r),
                y: -(hy - r),
            },
            Point {
                x: hx - r,
                y: hy - r,
            },
        ) - r)
            .max(0.);
    }
    ((q.x.abs() - hx).max(0.)).hypot((q.y.abs() - hy).max(0.))
}
