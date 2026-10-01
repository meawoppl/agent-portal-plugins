//! A small GEOS-faithful geometry kernel for the DRC port.
//!
//! Upstream's checker leans on shapely (GEOS 3.13). Where results are
//! reported (distances, locations, areas) this module reproduces GEOS's
//! algorithms operation-for-operation: buffer vertex generation
//! (`OffsetSegmentGenerator`), `DistanceOp` (containment then facet
//! distance, `LineSegment::closestPoints`), `Area::ofRing`,
//! `InteriorPointArea` (`representative_point`) and ray-crossing point
//! location. General polygon overlay (intersection / union / difference)
//! delegates to `geo`'s boolean ops; vertex positions of computed
//! intersection points can differ from GEOS in the last bits.

pub type C = (f64, f64);

/// Shapely `quad_segs` default.
pub const QUAD_SEGS: usize = 16;

/// Polygon with a closed shell and closed holes (first == last).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Poly {
    pub shell: Vec<C>,
    pub holes: Vec<Vec<C>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Geom {
    #[default]
    Empty,
    Point(C),
    Line(Vec<C>),
    Poly(Poly),
    Multi(Vec<Poly>),
    Lines(Vec<Vec<C>>),
    Collection(Vec<Geom>),
}

fn close(mut ring: Vec<C>) -> Vec<C> {
    if ring.first() != ring.last() {
        if let Some(&f) = ring.first() {
            ring.push(f);
        }
    }
    ring
}

impl Poly {
    pub fn new(shell: Vec<C>) -> Self {
        Poly {
            shell: close(shell),
            holes: Vec::new(),
        }
    }

    pub fn with_holes(shell: Vec<C>, holes: Vec<Vec<C>>) -> Self {
        Poly {
            shell: close(shell),
            holes: holes.into_iter().map(close).collect(),
        }
    }

    pub fn area(&self) -> f64 {
        let mut a = ring_area(&self.shell).abs();
        for h in &self.holes {
            a -= ring_area(h).abs();
        }
        a
    }

    pub fn is_empty(&self) -> bool {
        self.shell.is_empty()
    }

    fn rings(&self) -> impl Iterator<Item = &Vec<C>> {
        std::iter::once(&self.shell).chain(self.holes.iter())
    }
}

/// GEOS `Area::ofRingSigned` (shoelace relative to the first x).
pub fn ring_area(ring: &[C]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let x0 = ring[0].0;
    let mut sum = 0.0;
    for i in 1..n - 1 {
        let x = ring[i].0 - x0;
        let y1 = ring[i + 1].1;
        let y2 = ring[i - 1].1;
        sum += x * (y2 - y1);
    }
    sum / 2.0
}

pub type Bounds = (f64, f64, f64, f64);

fn extend(b: &mut Option<Bounds>, p: C) {
    *b = Some(match *b {
        None => (p.0, p.1, p.0, p.1),
        Some((a, c, d, e)) => (a.min(p.0), c.min(p.1), d.max(p.0), e.max(p.1)),
    });
}

impl Geom {
    pub fn is_empty(&self) -> bool {
        match self {
            Geom::Empty => true,
            Geom::Point(_) => false,
            Geom::Line(l) => l.is_empty(),
            Geom::Poly(p) => p.is_empty(),
            Geom::Multi(ps) => ps.iter().all(Poly::is_empty),
            Geom::Lines(ls) => ls.iter().all(Vec::is_empty),
            Geom::Collection(gs) => gs.iter().all(Geom::is_empty),
        }
    }

    pub fn polys(&self) -> Vec<&Poly> {
        match self {
            Geom::Poly(p) => vec![p],
            Geom::Multi(ps) => ps.iter().collect(),
            Geom::Collection(gs) => gs.iter().flat_map(|g| g.polys()).collect(),
            _ => vec![],
        }
    }

    /// Linear components in GEOS `LinearComponentExtracter` order.
    pub fn lines(&self) -> Vec<&[C]> {
        match self {
            Geom::Line(l) => vec![l.as_slice()],
            Geom::Lines(ls) => ls.iter().map(Vec::as_slice).collect(),
            Geom::Poly(p) => p.rings().map(Vec::as_slice).collect(),
            Geom::Multi(ps) => ps
                .iter()
                .flat_map(|p| p.rings().map(Vec::as_slice))
                .collect(),
            Geom::Collection(gs) => gs.iter().flat_map(|g| g.lines()).collect(),
            _ => vec![],
        }
    }

    pub fn points(&self) -> Vec<C> {
        match self {
            Geom::Point(p) => vec![*p],
            Geom::Collection(gs) => gs.iter().flat_map(|g| g.points()).collect(),
            _ => vec![],
        }
    }

    pub fn area(&self) -> f64 {
        self.polys().iter().map(|p| p.area()).sum::<f64>()
    }

    /// Envelope; `None` for empty.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut b = None;
        for p in self.points() {
            extend(&mut b, p);
        }
        for l in self.lines() {
            for &p in l {
                extend(&mut b, p);
            }
        }
        b
    }

    /// Total length of linear components / polygon rings.
    pub fn length(&self) -> f64 {
        let mut total = 0.0;
        for l in self.lines() {
            for w in l.windows(2) {
                total += dist(w[0], w[1]);
            }
        }
        total
    }

    /// Coordinates of `ConnectedElementLocationFilter` (first coordinate of
    /// each point / line / polygon component).
    fn element_locations(&self) -> Vec<C> {
        match self {
            Geom::Point(p) => vec![*p],
            Geom::Line(l) => l.first().copied().into_iter().collect(),
            Geom::Lines(ls) => ls.iter().filter_map(|l| l.first().copied()).collect(),
            Geom::Poly(p) => p.shell.first().copied().into_iter().collect(),
            Geom::Multi(ps) => ps.iter().filter_map(|p| p.shell.first().copied()).collect(),
            Geom::Collection(gs) => gs.iter().flat_map(|g| g.element_locations()).collect(),
            Geom::Empty => vec![],
        }
    }
}

#[inline]
pub fn dist(a: C, b: C) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

// ------------------------------------------------------------------ buffer

struct SegList {
    pts: Vec<C>,
    min_vertex_dist: f64,
}

impl SegList {
    fn new(distance: f64) -> Self {
        SegList {
            pts: Vec::new(),
            min_vertex_dist: distance * 1e-6,
        }
    }

    fn add(&mut self, p: C) {
        if let Some(&last) = self.pts.last() {
            if dist(last, p) < self.min_vertex_dist {
                return;
            }
        }
        self.pts.push(p);
    }

    fn close(mut self) -> Vec<C> {
        if let Some(&f) = self.pts.first() {
            if self.pts.last() != Some(&f) {
                self.pts.push(f);
            }
        }
        self.pts
    }
}

/// GEOS `addDirectedFillet` (clockwise when `cw`).
fn directed_fillet(sl: &mut SegList, p: C, start: f64, end: f64, cw: bool, r: f64, quad: usize) {
    let factor = if cw { -1.0 } else { 1.0 };
    let total = (start - end).abs();
    let quantum = std::f64::consts::FRAC_PI_2 / quad as f64;
    let n = (total / quantum + 0.5) as i64;
    if n < 1 {
        return;
    }
    let inc = total / n as f64;
    for i in 0..n {
        let a = start + factor * i as f64 * inc;
        sl.add((p.0 + r * a.cos(), p.1 + r * a.sin()));
    }
}

/// `Point(p).buffer(r)` (GEOS `createCircle`).
pub fn point_buffer(p: C, r: f64) -> Geom {
    point_buffer_q(p, r, QUAD_SEGS)
}

/// `Point(p).buffer(r, quad_segs=quad)`.
pub fn point_buffer_q(p: C, r: f64, quad: usize) -> Geom {
    if r <= 0.0 {
        return Geom::Empty;
    }
    let mut sl = SegList::new(r);
    sl.add((p.0 + r, p.1));
    directed_fillet(&mut sl, p, 0.0, 2.0 * std::f64::consts::PI, true, r, quad);
    Geom::Poly(Poly {
        shell: sl.close(),
        holes: vec![],
    })
}

fn offset_seg(a: C, b: C, d: f64, left: bool) -> (C, C) {
    let sign = if left { 1.0 } else { -1.0 };
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    let ux = sign * d * dx / len;
    let uy = sign * d * dy / len;
    ((a.0 - uy, a.1 + ux), (b.0 - uy, b.1 + ux))
}

fn round_cap(sl: &mut SegList, a: C, b: C, d: f64, quad: usize) {
    let (_, l1) = offset_seg(a, b, d, true);
    let (_, r1) = offset_seg(a, b, d, false);
    let angle = (b.1 - a.1).atan2(b.0 - a.0);
    sl.add(l1);
    directed_fillet(
        sl,
        b,
        angle + std::f64::consts::FRAC_PI_2,
        angle - std::f64::consts::FRAC_PI_2,
        true,
        d,
        quad,
    );
    sl.add(r1);
}

/// `LineString([a, b]).buffer(d)` with round caps.
pub fn segment_buffer(a: C, b: C, d: f64) -> Geom {
    segment_buffer_q(a, b, d, QUAD_SEGS)
}

/// `LineString([a, b]).buffer(d, quad_segs=quad)`.
pub fn segment_buffer_q(a: C, b: C, d: f64, quad: usize) -> Geom {
    if d <= 0.0 {
        return Geom::Empty;
    }
    if a == b {
        return point_buffer_q(a, d, quad);
    }
    let mut sl = SegList::new(d);
    let (_, l1) = offset_seg(a, b, d, true);
    sl.add(l1);
    round_cap(&mut sl, a, b, d, quad);
    let (_, rl1) = offset_seg(b, a, d, true);
    sl.add(rl1);
    round_cap(&mut sl, b, a, d, quad);
    Geom::Poly(Poly {
        shell: sl.close(),
        holes: vec![],
    })
}

/// `shapely.box(minx, miny, maxx, maxy)` (counter-clockwise).
pub fn box_poly(minx: f64, miny: f64, maxx: f64, maxy: f64) -> Geom {
    Geom::Poly(Poly::new(vec![
        (maxx, miny),
        (maxx, maxy),
        (minx, maxy),
        (minx, miny),
    ]))
}

/// `shapely.box(...).buffer(r, join_style=round)` for an axis-aligned
/// box, including the degenerate (zero width/height) boxes upstream's
/// oval/roundrect pads produce.
pub fn box_buffer(minx: f64, miny: f64, maxx: f64, maxy: f64, r: f64) -> Geom {
    if r <= 0.0 {
        return box_poly(minx, miny, maxx, maxy);
    }
    let w0 = maxx == minx;
    let h0 = maxy == miny;
    if w0 && h0 {
        return point_buffer((minx, miny), r);
    }
    if h0 {
        return segment_buffer((minx, miny), (maxx, miny), r);
    }
    if w0 {
        return segment_buffer((minx, maxy), (minx, miny), r);
    }
    use std::f64::consts::PI;
    let mut sl = SegList::new(r);
    // Clockwise, starting at the right end of the bottom edge offset.
    let corner = |sl: &mut SegList, c: C, from: C, to: C| {
        let mut start = (from.1 - c.1).atan2(from.0 - c.0);
        let end = (to.1 - c.1).atan2(to.0 - c.0);
        if start <= end {
            start += 2.0 * PI;
        }
        sl.add(from);
        directed_fillet(sl, c, start, end, true, r, QUAD_SEGS);
        sl.add(to);
    };
    sl.add((maxx, miny - r));
    corner(&mut sl, (minx, miny), (minx, miny - r), (minx - r, miny));
    corner(&mut sl, (minx, maxy), (minx - r, maxy), (minx, maxy + r));
    corner(&mut sl, (maxx, maxy), (maxx, maxy + r), (maxx + r, maxy));
    corner(&mut sl, (maxx, miny), (maxx + r, miny), (maxx, miny - r));
    Geom::Poly(Poly {
        shell: sl.close(),
        holes: vec![],
    })
}

/// `shapely.affinity.rotate(g, angle_deg, origin=(0, 0))`.
pub fn rotate(g: &Geom, angle_deg: f64) -> Geom {
    let a = angle_deg * std::f64::consts::PI / 180.0;
    let mut c = a.cos();
    let mut s = a.sin();
    if c.abs() < 2.5e-16 {
        c = 0.0;
    }
    if s.abs() < 2.5e-16 {
        s = 0.0;
    }
    map_coords(g, &|p| (c * p.0 - s * p.1, s * p.0 + c * p.1))
}

/// `shapely.affinity.translate(g, dx, dy)`.
pub fn translate(g: &Geom, dx: f64, dy: f64) -> Geom {
    map_coords(g, &|p| (p.0 + dx, p.1 + dy))
}

pub fn map_coords(g: &Geom, f: &dyn Fn(C) -> C) -> Geom {
    let ring = |r: &Vec<C>| r.iter().map(|&p| f(p)).collect::<Vec<C>>();
    let poly = |p: &Poly| Poly {
        shell: ring(&p.shell),
        holes: p.holes.iter().map(ring).collect(),
    };
    match g {
        Geom::Empty => Geom::Empty,
        Geom::Point(p) => Geom::Point(f(*p)),
        Geom::Line(l) => Geom::Line(ring(l)),
        Geom::Lines(ls) => Geom::Lines(ls.iter().map(ring).collect()),
        Geom::Poly(p) => Geom::Poly(poly(p)),
        Geom::Multi(ps) => Geom::Multi(ps.iter().map(poly).collect()),
        Geom::Collection(gs) => Geom::Collection(gs.iter().map(|g| map_coords(g, f)).collect()),
    }
}

// ---------------------------------------------------------------- location

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Interior,
    Boundary,
    Exterior,
}

fn orient(p1: C, p2: C, q: C) -> f64 {
    (p2.0 - p1.0) * (q.1 - p1.1) - (p2.1 - p1.1) * (q.0 - p1.0)
}

/// GEOS `RayCrossingCounter::locatePointInRing`.
pub fn locate_in_ring(p: C, ring: &[C]) -> Location {
    let mut crossings = 0usize;
    for i in 1..ring.len() {
        let p1 = ring[i];
        let p2 = ring[i - 1];
        if p1.0 < p.0 && p2.0 < p.0 {
            continue;
        }
        if p == p2 {
            return Location::Boundary;
        }
        if p1.1 == p.1 && p2.1 == p.1 {
            let (mn, mx) = if p1.0 < p2.0 { (p1.0, p2.0) } else { (p2.0, p1.0) };
            if p.0 >= mn && p.0 <= mx {
                return Location::Boundary;
            }
            continue;
        }
        if (p1.1 > p.1 && p2.1 <= p.1) || (p2.1 > p.1 && p1.1 <= p.1) {
            let mut o = orient(p1, p2, p).signum();
            if o == 0.0 {
                return Location::Boundary;
            }
            if p2.1 < p1.1 {
                o = -o;
            }
            if o > 0.0 {
                crossings += 1;
            }
        }
    }
    if crossings % 2 == 1 {
        Location::Interior
    } else {
        Location::Exterior
    }
}

pub fn locate_in_poly(p: C, poly: &Poly) -> Location {
    if poly.is_empty() {
        return Location::Exterior;
    }
    match locate_in_ring(p, &poly.shell) {
        Location::Exterior => return Location::Exterior,
        Location::Boundary => return Location::Boundary,
        Location::Interior => {}
    }
    for h in &poly.holes {
        match locate_in_ring(p, h) {
            Location::Interior => return Location::Exterior,
            Location::Boundary => return Location::Boundary,
            Location::Exterior => {}
        }
    }
    Location::Interior
}

/// Point `p` covered by (interior or boundary of) any polygon of `g`.
pub fn covers_point(g: &Geom, p: C) -> bool {
    g.polys()
        .iter()
        .any(|poly| locate_in_poly(p, poly) != Location::Exterior)
}

// ---------------------------------------------------------------- distance

/// GEOS `Distance::pointToSegment`.
pub fn point_to_segment(p: C, a: C, b: C) -> f64 {
    if a == b {
        return dist(p, a);
    }
    let len2 = (b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1);
    let r = ((p.0 - a.0) * (b.0 - a.0) + (p.1 - a.1) * (b.1 - a.1)) / len2;
    if r <= 0.0 {
        return dist(p, a);
    }
    if r >= 1.0 {
        return dist(p, b);
    }
    let s = ((a.1 - p.1) * (b.0 - a.0) - (a.0 - p.0) * (b.1 - a.1)) / len2;
    s.abs() * len2.sqrt()
}

fn env_intersects(a: C, b: C, c: C, d: C) -> bool {
    let (minq, maxq) = (c.0.min(d.0), c.0.max(d.0));
    let (minp, maxp) = (a.0.min(b.0), a.0.max(b.0));
    if minp > maxq || maxp < minq {
        return false;
    }
    let (minq, maxq) = (c.1.min(d.1), c.1.max(d.1));
    let (minp, maxp) = (a.1.min(b.1), a.1.max(b.1));
    !(minp > maxq || maxp < minq)
}

/// GEOS `Distance::segmentToSegment`.
pub fn segment_to_segment(a: C, b: C, c: C, d: C) -> f64 {
    if a == b {
        return point_to_segment(a, c, d);
    }
    if c == d {
        return point_to_segment(d, a, b);
    }
    let mut no_int = false;
    if !env_intersects(a, b, c, d) {
        no_int = true;
    } else {
        let denom = (b.0 - a.0) * (d.1 - c.1) - (b.1 - a.1) * (d.0 - c.0);
        if denom == 0.0 {
            no_int = true;
        } else {
            let r_num = (a.1 - c.1) * (d.0 - c.0) - (a.0 - c.0) * (d.1 - c.1);
            let s_num = (a.1 - c.1) * (b.0 - a.0) - (a.0 - c.0) * (b.1 - a.1);
            let s = s_num / denom;
            let r = r_num / denom;
            if !(0.0..=1.0).contains(&r) || !(0.0..=1.0).contains(&s) {
                no_int = true;
            }
        }
    }
    if no_int {
        return point_to_segment(a, c, d).min(
            point_to_segment(b, c, d).min(point_to_segment(c, a, b).min(point_to_segment(d, a, b))),
        );
    }
    0.0
}

fn projection_factor(p: C, a: C, b: C) -> f64 {
    if p == a {
        return 0.0;
    }
    if p == b {
        return 1.0;
    }
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len2 = dx * dx + dy * dy;
    if len2 <= 0.0 {
        return f64::NAN;
    }
    ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2
}

/// GEOS `LineSegment::closestPoint`.
pub fn closest_point_on_segment(p: C, a: C, b: C) -> C {
    let f = projection_factor(p, a, b);
    if f > 0.0 && f < 1.0 {
        if p == a || p == b {
            return p;
        }
        return (a.0 + f * (b.0 - a.0), a.1 + f * (b.1 - a.1));
    }
    if dist(a, p) < dist(b, p) {
        a
    } else {
        b
    }
}

/// Proper/endpoint intersection point of two segments, if any.
fn segment_intersection(a: C, b: C, c: C, d: C) -> Option<C> {
    let denom = (b.0 - a.0) * (d.1 - c.1) - (b.1 - a.1) * (d.0 - c.0);
    if denom == 0.0 {
        // Collinear overlap: return a shared endpoint if one lies on the other.
        for p in [c, d] {
            if point_to_segment(p, a, b) == 0.0 {
                return Some(p);
            }
        }
        for p in [a, b] {
            if point_to_segment(p, c, d) == 0.0 {
                return Some(p);
            }
        }
        return None;
    }
    let r = ((a.1 - c.1) * (d.0 - c.0) - (a.0 - c.0) * (d.1 - c.1)) / denom;
    let s = ((a.1 - c.1) * (b.0 - a.0) - (a.0 - c.0) * (b.1 - a.1)) / denom;
    if !(0.0..=1.0).contains(&r) || !(0.0..=1.0).contains(&s) {
        return None;
    }
    for p in [a, b, c, d] {
        if (p == a || p == b) && (p == c || p == d) {
            return Some(p);
        }
    }
    if r == 0.0 {
        return Some(a);
    }
    if r == 1.0 {
        return Some(b);
    }
    if s == 0.0 {
        return Some(c);
    }
    if s == 1.0 {
        return Some(d);
    }
    Some((a.0 + r * (b.0 - a.0), a.1 + r * (b.1 - a.1)))
}

/// GEOS `LineSegment::closestPoints` (`[on ab, on cd]`).
pub fn closest_points_segments(a: C, b: C, c: C, d: C) -> (C, C) {
    if let Some(p) = segment_intersection(a, b, c, d) {
        return (p, p);
    }
    let c00 = closest_point_on_segment(c, a, b);
    let mut min = dist(c00, c);
    let mut best = (c00, c);
    let c01 = closest_point_on_segment(d, a, b);
    let dd = dist(c01, d);
    if dd < min {
        min = dd;
        best = (c01, d);
    }
    let c10 = closest_point_on_segment(a, c, d);
    let dd = dist(c10, a);
    if dd < min {
        min = dd;
        best = (a, c10);
    }
    let c11 = closest_point_on_segment(b, c, d);
    let dd = dist(c11, b);
    if dd < min {
        best = (b, c11);
    }
    best
}

fn line_env(l: &[C]) -> Bounds {
    let mut b = None;
    for &p in l {
        extend(&mut b, p);
    }
    b.unwrap_or((0.0, 0.0, 0.0, 0.0))
}

fn env_distance(a: Bounds, b: Bounds) -> f64 {
    let dx = if a.2 < b.0 {
        b.0 - a.2
    } else if b.2 < a.0 {
        a.0 - b.2
    } else {
        0.0
    };
    let dy = if a.3 < b.1 {
        b.1 - a.3
    } else if b.3 < a.1 {
        a.1 - b.3
    } else {
        0.0
    };
    if dx == 0.0 {
        return dy;
    }
    if dy == 0.0 {
        return dx;
    }
    (dx * dx + dy * dy).sqrt()
}

/// GEOS `DistanceOp`: `(distance, nearest_on_g1, nearest_on_g2)`.
pub fn distance_points(g1: &Geom, g2: &Geom) -> Option<(f64, C, C)> {
    if g1.is_empty() || g2.is_empty() {
        return None;
    }
    // Containment.
    let polys2 = g2.polys();
    if !polys2.is_empty() {
        for p in g1.element_locations() {
            if polys2.iter().any(|poly| locate_in_poly(p, poly) != Location::Exterior) {
                return Some((0.0, p, p));
            }
        }
    }
    let polys1 = g1.polys();
    if !polys1.is_empty() {
        for p in g2.element_locations() {
            if polys1.iter().any(|poly| locate_in_poly(p, poly) != Location::Exterior) {
                return Some((0.0, p, p));
            }
        }
    }
    let mut min = f64::INFINITY;
    let mut loc = ((0.0, 0.0), (0.0, 0.0));
    let lines1 = g1.lines();
    let lines2 = g2.lines();
    'outer: for l1 in &lines1 {
        let e1 = line_env(l1);
        for l2 in &lines2 {
            if env_distance(e1, line_env(l2)) > min {
                continue;
            }
            for i in 0..l1.len().saturating_sub(1) {
                for j in 0..l2.len().saturating_sub(1) {
                    let d = segment_to_segment(l1[i], l1[i + 1], l2[j], l2[j + 1]);
                    if d < min {
                        min = d;
                        loc = closest_points_segments(l1[i], l1[i + 1], l2[j], l2[j + 1]);
                    }
                    if min <= 0.0 {
                        break 'outer;
                    }
                }
            }
        }
    }
    if min <= 0.0 {
        return Some((min, loc.0, loc.1));
    }
    let pts2 = g2.points();
    for l1 in &lines1 {
        for &p in &pts2 {
            for i in 0..l1.len().saturating_sub(1) {
                let d = point_to_segment(p, l1[i], l1[i + 1]);
                if d < min {
                    min = d;
                    loc = (closest_point_on_segment(p, l1[i], l1[i + 1]), p);
                }
            }
        }
    }
    let pts1 = g1.points();
    for l2 in &lines2 {
        for &p in &pts1 {
            for j in 0..l2.len().saturating_sub(1) {
                let d = point_to_segment(p, l2[j], l2[j + 1]);
                if d < min {
                    min = d;
                    loc = (p, closest_point_on_segment(p, l2[j], l2[j + 1]));
                }
            }
        }
    }
    for &p in &pts1 {
        for &q in &pts2 {
            let d = dist(p, q);
            if d < min {
                min = d;
                loc = (p, q);
            }
        }
    }
    Some((min, loc.0, loc.1))
}

/// `g1.distance(g2)` (0 for empty, like shapely returns 0.0 / nan).
pub fn distance(g1: &Geom, g2: &Geom) -> f64 {
    distance_points(g1, g2).map_or(f64::NAN, |d| d.0)
}

/// `shapely.ops.nearest_points(g1, g2)`.
pub fn nearest_points(g1: &Geom, g2: &Geom) -> (C, C) {
    distance_points(g1, g2).map_or(((0.0, 0.0), (0.0, 0.0)), |d| (d.1, d.2))
}

/// `g1.intersects(g2)`.
pub fn intersects(g1: &Geom, g2: &Geom) -> bool {
    match (g1.bounds(), g2.bounds()) {
        (Some(a), Some(b)) if env_distance(a, b) > 0.0 => return false,
        (None, _) | (_, None) => return false,
        _ => {}
    }
    distance_points(g1, g2).is_some_and(|d| d.0 == 0.0)
}

/// Distance from a point to the boundary of `g` (`g.boundary.distance(p)`).
pub fn boundary_distance(g: &Geom, p: C) -> f64 {
    let mut min = f64::INFINITY;
    for l in g.lines() {
        for w in l.windows(2) {
            let d = point_to_segment(p, w[0], w[1]);
            if d < min {
                min = d;
            }
        }
    }
    min
}

// ----------------------------------------------------- representative point

fn avg(a: f64, b: f64) -> f64 {
    (a + b) / 2.0
}

fn poly_interior_point(poly: &Poly) -> Option<(C, f64)> {
    if poly.is_empty() {
        return None;
    }
    let env = line_env(&poly.shell);
    let (mut lo, mut hi) = (env.1, env.3);
    let centre = avg(lo, hi);
    for r in poly.rings() {
        for &(_, y) in r {
            if y <= centre {
                if y > lo {
                    lo = y;
                }
            } else if y > centre && y < hi {
                hi = y;
            }
        }
    }
    let scan_y = avg(hi, lo);
    let mut crossings: Vec<f64> = Vec::new();
    for r in poly.rings() {
        let e = line_env(r);
        if scan_y < e.1 || scan_y > e.3 {
            continue;
        }
        for i in 1..r.len() {
            let (p0, p1) = (r[i - 1], r[i]);
            if (p0.1 > scan_y && p1.1 > scan_y) || (p0.1 < scan_y && p1.1 < scan_y) {
                continue;
            }
            if p0.1 == p1.1 {
                continue;
            }
            if p0.1 == scan_y && p1.1 < scan_y {
                continue;
            }
            if p1.1 == scan_y && p0.1 < scan_y {
                continue;
            }
            let x = if p0.0 == p1.0 {
                p0.0
            } else {
                let m = (p1.1 - p0.1) / (p1.0 - p0.0);
                p0.0 + ((scan_y - p0.1) / m)
            };
            crossings.push(x);
        }
    }
    let mut pt = poly.shell[0];
    let mut width = 0.0;
    if crossings.is_empty() {
        return Some((pt, width));
    }
    crossings.sort_by(f64::total_cmp);
    let mut i = 0;
    while i + 1 < crossings.len() {
        let (x1, x2) = (crossings[i], crossings[i + 1]);
        let w = x2 - x1;
        if w > width {
            width = w;
            pt = (avg(x1, x2), scan_y);
        }
        i += 2;
    }
    Some((pt, width))
}

/// `g.representative_point()` for polygonal geometry (GEOS
/// `InteriorPointArea`); falls back to the first coordinate otherwise.
pub fn representative_point(g: &Geom) -> Option<C> {
    let polys = g.polys();
    if !polys.is_empty() {
        let mut best: Option<C> = None;
        let mut max_w = -1.0;
        for p in polys {
            if let Some((pt, w)) = poly_interior_point(p) {
                if w > max_w {
                    max_w = w;
                    best = Some(pt);
                }
            }
        }
        return best;
    }
    // Lines: GEOS InteriorPointLine picks an interior vertex nearest the
    // centroid; points: the point nearest the centroid.
    let lines = g.lines();
    if !lines.is_empty() {
        let (mut cx, mut cy, mut tl) = (0.0, 0.0, 0.0);
        for l in &lines {
            for w in l.windows(2) {
                let len = dist(w[0], w[1]);
                cx += len * (w[0].0 + w[1].0) / 2.0;
                cy += len * (w[0].1 + w[1].1) / 2.0;
                tl += len;
            }
        }
        let c = if tl > 0.0 { (cx / tl, cy / tl) } else { lines[0][0] };
        let mut best = None;
        let mut bd = f64::INFINITY;
        for l in &lines {
            for &p in &l[1..l.len().saturating_sub(1)] {
                let d = dist(p, c);
                if d < bd {
                    bd = d;
                    best = Some(p);
                }
            }
        }
        if best.is_none() {
            for l in &lines {
                for &p in [l[0], l[l.len() - 1]].iter() {
                    let d = dist(p, c);
                    if d < bd {
                        bd = d;
                        best = Some(p);
                    }
                }
            }
        }
        return best;
    }
    g.points().first().copied()
}

// ---------------------------------------------------------------- overlay

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::Solver;
use i_overlay::float::overlay::{FloatOverlay, OverlayOptions};

type Shapes = Vec<Vec<Vec<[f64; 2]>>>;

fn open_ring(r: &[C]) -> Vec<[f64; 2]> {
    let n = if r.len() > 1 && r.first() == r.last() {
        r.len() - 1
    } else {
        r.len()
    };
    r[..n].iter().map(|p| [p.0, p.1]).collect()
}

fn to_shapes(g: &Geom) -> Shapes {
    g.polys()
        .iter()
        .filter(|p| p.shell.len() >= 3)
        .map(|p| {
            std::iter::once(open_ring(&p.shell))
                .chain(p.holes.iter().filter(|h| h.len() >= 3).map(|h| open_ring(h)))
                .collect()
        })
        .collect()
}

/// Snap overlay output vertices back onto exactly representable input
/// vertices (the overlay engine works on a fixed-point grid).
struct Snapper {
    cell: f64,
    grid: std::collections::HashMap<(i64, i64), Vec<C>>,
}

impl Snapper {
    fn new(inputs: &[&Shapes]) -> Self {
        let cell = 1e-6;
        let mut grid: std::collections::HashMap<(i64, i64), Vec<C>> = Default::default();
        for s in inputs {
            for shape in s.iter() {
                for ring in shape {
                    for p in ring {
                        let k = ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
                        grid.entry(k).or_default().push((p[0], p[1]));
                    }
                }
            }
        }
        Snapper { cell, grid }
    }

    fn snap(&self, p: [f64; 2]) -> C {
        let (kx, ky) = ((p[0] / self.cell).floor() as i64, (p[1] / self.cell).floor() as i64);
        let mut best = (p[0], p[1]);
        let mut bd = self.cell;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(v) = self.grid.get(&(kx + dx, ky + dy)) {
                    for &q in v {
                        let d = dist(q, (p[0], p[1]));
                        if d < bd {
                            bd = d;
                            best = q;
                        }
                    }
                }
            }
        }
        best
    }
}

fn from_shapes(out: Shapes, snap: &Snapper) -> Geom {
    let ring = |r: Vec<[f64; 2]>| close(r.into_iter().map(|p| snap.snap(p)).collect());
    let polys: Vec<Poly> = out
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|mut s| {
            let shell = ring(s.remove(0));
            Poly {
                shell,
                holes: s.into_iter().map(ring).collect(),
            }
        })
        .filter(|p| p.shell.len() >= 4)
        .collect();
    match polys.len() {
        0 => Geom::Empty,
        1 => Geom::Poly(polys.into_iter().next().unwrap()),
        _ => Geom::Multi(polys),
    }
}

fn run_overlay(subj: &Shapes, clip: &Shapes, rule: OverlayRule) -> Geom {
    let snap = Snapper::new(&[subj, clip]);
    if subj.is_empty() && clip.is_empty() {
        return Geom::Empty;
    }
    let opts = OverlayOptions::<f64> {
        preserve_input_collinear: true,
        preserve_output_collinear: true,
        ..Default::default()
    };
    let out = FloatOverlay::with_subj_and_clip_custom(subj, clip, opts, Solver::default())
        .overlay(rule, FillRule::EvenOdd);
    from_shapes(out, &snap)
}

/// Sutherland-Hodgman crop of one ring to an axis-aligned box.
fn crop_ring(ring: &[[f64; 2]], b: Bounds) -> Vec<[f64; 2]> {
    let mut pts = ring.to_vec();
    for edge in 0..4 {
        if pts.is_empty() {
            break;
        }
        let inside = |p: &[f64; 2]| match edge {
            0 => p[0] >= b.0,
            1 => p[0] <= b.2,
            2 => p[1] >= b.1,
            _ => p[1] <= b.3,
        };
        let cut = |p: &[f64; 2], q: &[f64; 2]| -> [f64; 2] {
            match edge {
                0 | 1 => {
                    let x = if edge == 0 { b.0 } else { b.2 };
                    let t = (x - p[0]) / (q[0] - p[0]);
                    [x, p[1] + t * (q[1] - p[1])]
                }
                _ => {
                    let y = if edge == 2 { b.1 } else { b.3 };
                    let t = (y - p[1]) / (q[1] - p[1]);
                    [p[0] + t * (q[0] - p[0]), y]
                }
            }
        };
        let mut out = Vec::with_capacity(pts.len());
        for i in 0..pts.len() {
            let cur = &pts[i];
            let prev = &pts[(i + pts.len() - 1) % pts.len()];
            match (inside(prev), inside(cur)) {
                (true, true) => out.push(*cur),
                (true, false) => out.push(cut(prev, cur)),
                (false, true) => {
                    out.push(cut(prev, cur));
                    out.push(*cur);
                }
                (false, false) => {}
            }
        }
        pts = out;
    }
    pts
}

fn crop_shapes(s: &Shapes, b: Bounds) -> Shapes {
    s.iter()
        .filter_map(|shape| {
            let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
            for (i, r) in shape.iter().enumerate() {
                let c = crop_ring(r, b);
                if c.len() >= 3 {
                    rings.push(c);
                } else if i == 0 {
                    return None;
                }
            }
            Some(rings)
        })
        .collect()
}

/// Polygonal intersection (areal part only).
pub fn intersection(a: &Geom, b: &Geom) -> Geom {
    let (Some(ba), Some(bb)) = (a.bounds(), b.bounds()) else {
        return Geom::Empty;
    };
    if a.polys().is_empty() || b.polys().is_empty() {
        return Geom::Empty;
    }
    let bx = (ba.0.max(bb.0), ba.1.max(bb.1), ba.2.min(bb.2), ba.3.min(bb.3));
    if bx.0 > bx.2 || bx.1 > bx.3 {
        return Geom::Empty;
    }
    let pad = 1e-9 * (1.0 + bx.2.abs().max(bx.3.abs()));
    let bx = (bx.0 - pad, bx.1 - pad, bx.2 + pad, bx.3 + pad);
    let sa = crop_shapes(&to_shapes(a), bx);
    let sb = crop_shapes(&to_shapes(b), bx);
    if sa.is_empty() || sb.is_empty() {
        return Geom::Empty;
    }
    run_overlay(&sa, &sb, OverlayRule::Intersect)
}

/// Polygonal union.
pub fn union(a: &Geom, b: &Geom) -> Geom {
    run_overlay(&to_shapes(a), &to_shapes(b), OverlayRule::Union)
}

/// Union of many polygonal geometries.
pub fn unary_union(gs: &[Geom]) -> Geom {
    let mut all: Shapes = Vec::new();
    for g in gs {
        all.extend(to_shapes(g));
    }
    if all.is_empty() {
        return Geom::Empty;
    }
    let snap = Snapper::new(&[&all]);
    let opts = OverlayOptions::<f64> {
        preserve_input_collinear: true,
        preserve_output_collinear: true,
        ..Default::default()
    };
    let out = FloatOverlay::with_subj_custom(&all, opts, Solver::default())
        .overlay(OverlayRule::Subject, FillRule::NonZero);
    from_shapes(out, &snap)
}

/// Polygonal difference `a - b`.
pub fn difference(a: &Geom, b: &Geom) -> Geom {
    if b.polys().is_empty() {
        return a.clone();
    }
    run_overlay(&to_shapes(a), &to_shapes(b), OverlayRule::Difference)
}

/// Line-segment intersection with a polygonal geometry: the clipped
/// pieces of `[a, b]` inside `g` (for `LineString.intersection(poly)`).
pub fn clip_segment(a: C, b: C, g: &Geom) -> Vec<(C, C)> {
    let mut ts = vec![0.0, 1.0];
    for l in g.lines() {
        for w in l.windows(2) {
            let (c, d) = (w[0], w[1]);
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
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup();
    let at = |t: f64| (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
    let mut out: Vec<(C, C)> = Vec::new();
    for w in ts.windows(2) {
        let mid = at((w[0] + w[1]) / 2.0);
        if covers_point(g, mid) {
            let p0 = if w[0] == 0.0 { a } else { at(w[0]) };
            let p1 = if w[1] == 1.0 { b } else { at(w[1]) };
            match out.last_mut() {
                Some(last) if last.1 == p0 => last.1 = p1,
                _ => out.push((p0, p1)),
            }
        }
    }
    out
}

/// Repair an invalid polygon (`make_valid`, linework/even-odd parity),
/// polygonal parts only.
pub fn make_valid(p: &Poly) -> Geom {
    let shapes = to_shapes(&Geom::Poly(p.clone()));
    let snap = Snapper::new(&[&shapes]);
    let opts = OverlayOptions::<f64> {
        preserve_input_collinear: true,
        preserve_output_collinear: true,
        ..Default::default()
    };
    let out = FloatOverlay::with_subj_custom(&shapes, opts, Solver::default())
        .overlay(OverlayRule::Subject, FillRule::EvenOdd);
    from_shapes(out, &snap)
}

/// GEOS-style validity check for ring polygons: enough distinct points
/// and no self-intersection / self-touch between non-adjacent edges
/// (repeated consecutive points are ignored, as GEOS does).
pub fn is_valid(p: &Poly) -> bool {
    p.rings().all(|r| ring_is_simple(r))
}

fn ring_is_simple(r: &[C]) -> bool {
    let mut pts: Vec<C> = Vec::with_capacity(r.len());
    for &q in r {
        if pts.last() != Some(&q) {
            pts.push(q);
        }
    }
    if pts.len() < 4 || pts.first() != pts.last() {
        return false;
    }
    let n = pts.len() - 1;
    let mut order: Vec<usize> = (0..n).collect();
    let lo = |i: usize| pts[i].0.min(pts[i + 1].0);
    let hi = |i: usize| pts[i].0.max(pts[i + 1].0);
    order.sort_by(|&a, &b| lo(a).total_cmp(&lo(b)));
    for (k, &i) in order.iter().enumerate() {
        let reach = hi(i);
        for &j in &order[k + 1..] {
            if lo(j) > reach {
                break;
            }
            let (a, b) = if i < j { (i, j) } else { (j, i) };
            if b == a + 1 || (a == 0 && b == n - 1) {
                // Adjacent edges share exactly one vertex; they are only
                // invalid when they overlap (a spike).
                let shared = if b == a + 1 { pts[b] } else { pts[0] };
                let (p_far, q_far) = if b == a + 1 {
                    (pts[a], pts[b + 1])
                } else {
                    (pts[1], pts[n - 1])
                };
                if orient(p_far, shared, q_far) == 0.0 {
                    let d1 = (p_far.0 - shared.0, p_far.1 - shared.1);
                    let d2 = (q_far.0 - shared.0, q_far.1 - shared.1);
                    if d1.0 * d2.0 + d1.1 * d2.1 > 0.0 {
                        return false;
                    }
                }
                continue;
            }
            if segment_to_segment(pts[a], pts[a + 1], pts[b], pts[b + 1]) == 0.0 {
                return false;
            }
        }
    }
    true
}

/// Negative buffer (`buffer(-d)`) of a convex polygon: each edge offset
/// inward by `d`, consecutive offset lines intersected. Returns `Empty`
/// when the polygon collapses.
pub fn erode_convex(g: &Geom, d: f64) -> Geom {
    let Geom::Poly(p) = g else {
        return Geom::Empty;
    };
    let mut ring: Vec<C> = Vec::new();
    for &q in &p.shell {
        if ring.last() != Some(&q) {
            ring.push(q);
        }
    }
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    let n = ring.len();
    if n < 3 {
        return Geom::Empty;
    }
    let area = ring_area(&p.shell);
    // Inward normal side: left for clockwise-by-shapely-sign rings.
    let ccw = area < 0.0; // GEOS ofRingSigned is positive for CW rings.
    let lines: Vec<(C, C)> = (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (o0, o1) = offset_seg(a, b, d, ccw);
            (o0, o1)
        })
        .collect();
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..n {
        let (a0, a1) = lines[(i + n - 1) % n];
        let (b0, b1) = lines[i];
        let den = (a1.0 - a0.0) * (b1.1 - b0.1) - (a1.1 - a0.1) * (b1.0 - b0.0);
        if den == 0.0 {
            out.push(b0);
            continue;
        }
        let t = ((b0.0 - a0.0) * (b1.1 - b0.1) - (b0.1 - a0.1) * (b1.0 - b0.0)) / den;
        out.push((a0.0 + t * (a1.0 - a0.0), a0.1 + t * (a1.1 - a0.1)));
    }
    out.push(out[0]);
    let new_area = ring_area(&out);
    if new_area == 0.0 || (new_area < 0.0) != (area < 0.0) || new_area.abs() >= area.abs() {
        return Geom::Empty;
    }
    // Every original edge must survive with positive length in the same
    // direction, otherwise the offset polygon inverted locally.
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let (c, e) = (out[i], out[i + 1]);
        if (b.0 - a.0) * (e.0 - c.0) + (b.1 - a.1) * (e.1 - c.1) <= 0.0 {
            return Geom::Empty;
        }
    }
    Geom::Poly(Poly {
        shell: out,
        holes: vec![],
    })
}

/// Disc with a concentric hole (annulus); plain disc when `inner <= 0`.
pub fn annulus(center: C, outer: f64, inner: f64, quad: usize) -> Geom {
    let Geom::Poly(mut o) = point_buffer_q(center, outer, quad) else {
        return Geom::Empty;
    };
    if inner > 0.0 {
        if let Geom::Poly(h) = point_buffer_q(center, inner, quad) {
            let mut hole = h.shell;
            hole.reverse();
            o.holes.push(hole);
        }
    }
    Geom::Poly(o)
}

// ------------------------------------------------------------- prepared

/// Uniform-grid segment index over one linear component.
#[derive(Debug, Clone)]
struct LineIndex {
    min: C,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl LineIndex {
    fn new(l: &[C]) -> Self {
        let b = line_env(l);
        let nseg = l.len().saturating_sub(1).max(1);
        let w = (b.2 - b.0).max(1e-9);
        let h = (b.3 - b.1).max(1e-9);
        let target = (nseg as f64 / 2.0).max(1.0);
        let cell = ((w * h) / target).sqrt().max(w.max(h) / 1024.0).max(1e-9);
        let nx = ((w / cell).floor() as usize + 1).min(4096);
        let ny = ((h / cell).floor() as usize + 1).min(4096);
        let mut cells = vec![Vec::new(); nx * ny];
        let idx = LineIndex {
            min: (b.0, b.1),
            cell,
            nx,
            ny,
            cells: Vec::new(),
        };
        for i in 0..l.len().saturating_sub(1) {
            let (a, c) = (l[i], l[i + 1]);
            let (x0, x1) = idx.col_range(a.0.min(c.0), a.0.max(c.0));
            let (y0, y1) = idx.row_range(a.1.min(c.1), a.1.max(c.1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[y * nx + x].push(i as u32);
                }
            }
        }
        LineIndex { cells, ..idx }
    }

    fn col_range(&self, lo: f64, hi: f64) -> (usize, usize) {
        let f = |v: f64| (((v - self.min.0) / self.cell).floor().max(0.0) as usize).min(self.nx - 1);
        (f(lo), f(hi))
    }

    fn row_range(&self, lo: f64, hi: f64) -> (usize, usize) {
        let f = |v: f64| (((v - self.min.1) / self.cell).floor().max(0.0) as usize).min(self.ny - 1);
        (f(lo), f(hi))
    }

    /// Segment indices whose bbox may intersect `b`, ascending, deduped.
    fn query(&self, b: Bounds, out: &mut Vec<u32>) {
        out.clear();
        let (x0, x1) = self.col_range(b.0, b.2);
        let (y0, y1) = self.row_range(b.1, b.3);
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend_from_slice(&self.cells[y * self.nx + x]);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// A geometry with per-ring segment indexes for repeated distance /
/// intersects / point-location queries (results identical to the plain
/// functions, including nearest-point tie-breaking).
#[derive(Debug, Clone)]
pub struct Prepared {
    pub geom: Geom,
    lines: Vec<LineIndex>,
    envs: Vec<Bounds>,
    bounds: Option<Bounds>,
}

impl Prepared {
    pub fn new(geom: Geom) -> Self {
        let lines = geom.lines().iter().map(|l| LineIndex::new(l)).collect();
        let envs = geom.lines().iter().map(|l| line_env(l)).collect();
        let bounds = geom.bounds();
        Prepared {
            geom,
            lines,
            envs,
            bounds,
        }
    }

    pub fn bounds(&self) -> Option<Bounds> {
        self.bounds
    }

    fn locate_ring(&self, li: usize, ring: &[C], p: C, buf: &mut Vec<u32>) -> Location {
        let idx = &self.lines[li];
        let e = line_env(ring);
        if p.1 < e.1 || p.1 > e.3 || p.0 > e.2 {
            return Location::Exterior;
        }
        idx.query((p.0, p.1, e.2, p.1), buf);
        let mut crossings = 0usize;
        for &k in buf.iter() {
            let i = k as usize + 1;
            let p1 = ring[i];
            let p2 = ring[i - 1];
            if p1.0 < p.0 && p2.0 < p.0 {
                continue;
            }
            if p == p2 || p == p1 {
                return Location::Boundary;
            }
            if p1.1 == p.1 && p2.1 == p.1 {
                let (mn, mx) = if p1.0 < p2.0 { (p1.0, p2.0) } else { (p2.0, p1.0) };
                if p.0 >= mn && p.0 <= mx {
                    return Location::Boundary;
                }
                continue;
            }
            if (p1.1 > p.1 && p2.1 <= p.1) || (p2.1 > p.1 && p1.1 <= p.1) {
                let mut o = orient(p1, p2, p).signum();
                if o == 0.0 {
                    return Location::Boundary;
                }
                if p2.1 < p1.1 {
                    o = -o;
                }
                if o > 0.0 {
                    crossings += 1;
                }
            }
        }
        if crossings % 2 == 1 {
            Location::Interior
        } else {
            Location::Exterior
        }
    }

    /// Point covered by (interior or boundary of) the polygonal parts.
    pub fn covers_point(&self, p: C) -> bool {
        let mut buf = Vec::new();
        let mut li = 0;
        for poly in self.geom.polys() {
            let n = 1 + poly.holes.len();
            if !poly.is_empty() {
                match self.locate_ring(li, &poly.shell, p, &mut buf) {
                    Location::Boundary => return true,
                    Location::Exterior => {}
                    Location::Interior => {
                        let mut inside = true;
                        for (k, h) in poly.holes.iter().enumerate() {
                            match self.locate_ring(li + 1 + k, h, p, &mut buf) {
                                Location::Interior => {
                                    inside = false;
                                    break;
                                }
                                Location::Boundary => return true,
                                Location::Exterior => {}
                            }
                        }
                        if inside {
                            return true;
                        }
                    }
                }
            }
            li += n;
        }
        false
    }
}

/// `distance_points(a, b)` with `b` prepared.
pub fn distance_points_prep(a: &Geom, b: &Prepared) -> Option<(f64, C, C)> {
    if a.is_empty() || b.geom.is_empty() {
        return None;
    }
    if !b.geom.polys().is_empty() {
        for p in a.element_locations() {
            if b.covers_point(p) {
                return Some((0.0, p, p));
            }
        }
    }
    let polys1 = a.polys();
    if !polys1.is_empty() {
        for p in b.geom.element_locations() {
            if polys1
                .iter()
                .any(|poly| locate_in_poly(p, poly) != Location::Exterior)
            {
                return Some((0.0, p, p));
            }
        }
    }
    let lines1 = a.lines();
    let lines2 = b.geom.lines();
    let pts1 = a.points();
    let ab = a.bounds()?;
    let bb = b.bounds?;
    let mut r = env_distance(ab, bb)
        .max(0.5 * (ab.2 - ab.0).max(ab.3 - ab.1))
        .max(0.05);
    let span = (bb.2 - bb.0)
        .max(bb.3 - bb.1)
        .max(ab.2 - ab.0)
        .max(ab.3 - ab.1);
    let mut buf: Vec<u32> = Vec::new();
    loop {
        let q = (ab.0 - r, ab.1 - r, ab.2 + r, ab.3 + r);
        let mut min = f64::INFINITY;
        let mut loc = ((0.0, 0.0), (0.0, 0.0));
        let mut found_zero = false;
        'outer: for l1 in &lines1 {
            let e1 = line_env(l1);
            for (li, l2) in lines2.iter().enumerate() {
                let e2 = b.envs[li];
                if env_distance(q, e2) > 0.0 || env_distance(e1, e2) > min {
                    continue;
                }
                b.lines[li].query(q, &mut buf);
                if buf.is_empty() {
                    continue;
                }
                for i in 0..l1.len().saturating_sub(1) {
                    for &j in buf.iter() {
                        let j = j as usize;
                        let d = segment_to_segment(l1[i], l1[i + 1], l2[j], l2[j + 1]);
                        if d < min {
                            min = d;
                            loc = closest_points_segments(l1[i], l1[i + 1], l2[j], l2[j + 1]);
                        }
                        if min <= 0.0 {
                            found_zero = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        if found_zero {
            return Some((min, loc.0, loc.1));
        }
        for &p in &pts1 {
            for (li, l2) in lines2.iter().enumerate() {
                if env_distance(q, b.envs[li]) > 0.0 {
                    continue;
                }
                b.lines[li].query(q, &mut buf);
                for &j in buf.iter() {
                    let j = j as usize;
                    let d = point_to_segment(p, l2[j], l2[j + 1]);
                    if d < min {
                        min = d;
                        loc = (p, closest_point_on_segment(p, l2[j], l2[j + 1]));
                    }
                }
            }
        }
        if min <= r || r > 4.0 * span + 1.0 {
            if min.is_infinite() {
                return distance_points(a, &b.geom);
            }
            return Some((min, loc.0, loc.1));
        }
        r *= 4.0;
    }
}

pub fn distance_prep(a: &Geom, b: &Prepared) -> f64 {
    distance_points_prep(a, b).map_or(f64::NAN, |d| d.0)
}

pub fn intersects_prep(a: &Geom, b: &Prepared) -> bool {
    match (a.bounds(), b.bounds) {
        (Some(x), Some(y)) if env_distance(x, y) > 0.0 => return false,
        (None, _) | (_, None) => return false,
        _ => {}
    }
    distance_points_prep(a, b).is_some_and(|d| d.0 == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_matches_geos() {
        let Geom::Poly(p) = point_buffer((0.0, 0.0), 1.0) else {
            panic!()
        };
        assert_eq!(p.shell.len(), 65);
        assert_eq!(p.shell[1], (0.9951847266721969, -0.0980171403295606));
        assert_eq!(p.shell[63], (0.9951847266721969, 0.0980171403295605));
    }

    #[test]
    fn capsule_matches_geos() {
        let Geom::Poly(p) = segment_buffer((0.0, 0.0), (2.0, 0.0), 0.5) else {
            panic!()
        };
        assert_eq!(p.shell.len(), 67);
        assert_eq!(p.shell[0], (2.0, 0.5));
        assert_eq!(p.shell[1], (2.0490085701647804, 0.4975923633360984));
        let Geom::Poly(p) = segment_buffer((0.0, 0.0), (2.0, 1.0), 0.5) else {
            panic!()
        };
        assert_eq!(p.shell[0], (1.776393202250021, 1.4472135954999579));
    }

    #[test]
    fn roundrect_matches_geos() {
        let Geom::Poly(p) = box_buffer(-1.0, -0.5, 1.0, 0.5, 0.25) else {
            panic!()
        };
        assert_eq!(p.shell.len(), 69);
        assert_eq!(p.shell[0], (1.0, -0.75));
        assert_eq!(p.shell[2], (-1.0245042850823902, -0.7487961816680493));
        assert_eq!(p.shell[17], (-1.25, -0.5));
    }
}
