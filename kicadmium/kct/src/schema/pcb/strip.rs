//! `PCB.strip_traces`: remove routing by net / layer / power / region.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::sexp::SExp;

use super::util::{gf, gi, gs, is_power_net, string_atoms, xy};
use super::{quant, Pcb, Point};

/// `strip_traces` keyword arguments (upstream defaults: everything, zones
/// kept, power not excluded, no orphan pass, unbounded).
pub struct StripOptions<'a> {
    /// Only these net names (`None` = all nets).
    pub nets: Option<Vec<String>>,
    /// Only segments on these layers; vias only when all their layers match.
    pub layers: Option<Vec<String>>,
    pub keep_zones: bool,
    /// Never strip power/ground nets.
    pub exclude_power: bool,
    /// Overrides the built-in power-net heuristic.
    pub power_pattern: Option<&'a dyn Fn(&str) -> bool>,
    /// After layer stripping, remove vias with no remaining segment
    /// endpoint on any of their layers.
    pub remove_orphan_vias: bool,
    /// Board-relative `(x1, y1, x2, y2)`; inside geometry is removed,
    /// crossing segments are clipped to their outside piece.
    pub region: Option<(f64, f64, f64, f64)>,
}

impl Default for StripOptions<'_> {
    fn default() -> Self {
        StripOptions {
            nets: None,
            layers: None,
            keep_zones: true,
            exclude_power: false,
            power_pattern: None,
            remove_orphan_vias: false,
            region: None,
        }
    }
}

/// Counts of removed / clipped / skipped elements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StripStats {
    pub segments: usize,
    pub vias: usize,
    pub zones: usize,
    pub segments_clipped: usize,
    pub segments_boundary_skipped: usize,
}

/// Liang-Barsky: does `start -> end` cross the normalized box?
pub(crate) fn segment_span_intersects_region(
    start: Point,
    end: Point,
    region: (f64, f64, f64, f64),
) -> bool {
    let (x1, y1, x2, y2) = region;
    let (sx, sy) = start;
    let (dx, dy) = (end.0 - sx, end.1 - sy);
    if dx == 0.0 && dy == 0.0 {
        return x1 <= sx && sx <= x2 && y1 <= sy && sy <= y2;
    }
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, sx - x1), (dx, x2 - sx), (-dy, sy - y1), (dy, y2 - sy)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return false;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return false;
            }
            if r < t1 {
                t1 = r;
            }
        }
    }
    t0 <= t1
}

impl Pcb {
    /// Remove trace segments and vias (and optionally zones) while keeping
    /// placement. Filters (`nets`, `layers`, `exclude_power`, `region`) are
    /// ANDed. Name-based `(net "GND")` refs resolve via the header table.
    pub fn strip_traces(&mut self, opts: StripOptions<'_>) -> StripStats {
        let net_numbers: Option<HashSet<i64>> = opts.nets.as_ref().map(|names| {
            names
                .iter()
                .filter_map(|n| self.nets.iter().find(|x| &x.name == n).map(|x| x.number))
                .collect()
        });
        let name_to_number: HashMap<String, i64> = self
            .nets
            .iter()
            .filter(|n| !n.name.is_empty() && n.number != 0)
            .map(|n| (n.name.clone(), n.number))
            .collect();
        let layer_set: Option<HashSet<String>> =
            opts.layers.as_ref().map(|l| l.iter().cloned().collect());
        let region = opts
            .region
            .map(|(a, b, c, d)| (a.min(c), b.min(d), a.max(c), b.max(d)));
        let (ox, oy) = self.board_origin;
        let in_region = |x: f64, y: f64| match region {
            None => true,
            Some((x1, y1, x2, y2)) => {
                let (rx, ry) = (x - ox, y - oy);
                x1 <= rx && rx <= x2 && y1 <= ry && ry <= y2
            }
        };
        let clip = |inside: Point, outside: Point| -> Point {
            let (x1, y1, x2, y2) = region.unwrap();
            let (ix, iy) = (inside.0 - ox, inside.1 - oy);
            let (px, py) = (outside.0 - ox, outside.1 - oy);
            let (dx, dy) = (px - ix, py - iy);
            let mut t = 1.0f64;
            if dx > 0.0 {
                t = t.min((x2 - ix) / dx);
            } else if dx < 0.0 {
                t = t.min((x1 - ix) / dx);
            }
            if dy > 0.0 {
                t = t.min((y2 - iy) / dy);
            } else if dy < 0.0 {
                t = t.min((y1 - iy) / dy);
            }
            let t = t.clamp(0.0, 1.0);
            (ix + dx * t + ox, iy + dy * t + oy)
        };
        let power: HashSet<i64> = if opts.exclude_power {
            self.nets
                .iter()
                .filter(|n| is_power_net(&n.name, opts.power_pattern))
                .map(|n| n.number)
                .collect()
        } else {
            HashSet::new()
        };
        let net_of = |child: &SExp| -> i64 {
            let Some(net) = child.find("net") else {
                return 0;
            };
            if let Some(n) = gi(net, 0) {
                return n;
            }
            gs(net, 0)
                .and_then(|name| name_to_number.get(&name).copied())
                .unwrap_or(0)
        };
        let passes_net = |n: i64| {
            if net_numbers.as_ref().is_some_and(|s| !s.contains(&n)) {
                return false;
            }
            !(opts.exclude_power && power.contains(&n))
        };

        let mut stats = StripStats::default();
        let mut clipped: HashMap<String, (Point, Point)> = HashMap::new();
        let children = std::mem::take(&mut self.doc.root.children);
        let mut kept = Vec::with_capacity(children.len());
        for mut child in children {
            let mut remove = false;
            if child.has_tag("segment") {
                let pn = passes_net(net_of(&child));
                let pl = !pn
                    || layer_set.as_ref().is_none_or(|ls| {
                        ls.contains(
                            &child
                                .find("layer")
                                .and_then(|l| gs(l, 0))
                                .unwrap_or_default(),
                        )
                    });
                if pn && pl {
                    if let Some(rbox) = region {
                        if let (Some(s), Some(e)) =
                            (child.find("start").map(xy), child.find("end").map(xy))
                        {
                            let (si, ei) = (in_region(s.0, s.1), in_region(e.0, e.1));
                            if si && ei {
                                remove = true;
                            } else if si != ei {
                                let (inside, outside) = if si { (s, e) } else { (e, s) };
                                let c = clip(inside, outside);
                                let tag = if si { "start" } else { "end" };
                                if let Some(n) = child.get_mut(tag) {
                                    n.set_value(0, c.0);
                                    n.set_value(1, c.1);
                                }
                                let (abs_s, abs_e) = if si { (c, e) } else { (s, c) };
                                stats.segments_clipped += 1;
                                if let Some(u) = child.find("uuid").and_then(|u| gs(u, 0)) {
                                    if !u.is_empty() {
                                        clipped.insert(
                                            u,
                                            (
                                                (abs_s.0 - ox, abs_s.1 - oy),
                                                (abs_e.0 - ox, abs_e.1 - oy),
                                            ),
                                        );
                                    }
                                }
                            } else if segment_span_intersects_region(
                                (s.0 - ox, s.1 - oy),
                                (e.0 - ox, e.1 - oy),
                                rbox,
                            ) {
                                stats.segments_boundary_skipped += 1;
                            }
                        }
                    } else {
                        remove = true;
                    }
                }
                if remove {
                    stats.segments += 1;
                }
            } else if child.has_tag("via") {
                if passes_net(net_of(&child)) {
                    remove = match &layer_set {
                        Some(ls) => child.find("layers").is_some_and(|l| {
                            let vl = string_atoms(l);
                            !vl.is_empty() && vl.iter().all(|x| ls.contains(x))
                        }),
                        None => true,
                    };
                }
                if remove && region.is_some() {
                    remove = child
                        .find("at")
                        .map(xy)
                        .is_some_and(|(x, y)| in_region(x, y));
                }
                if remove {
                    stats.vias += 1;
                }
            } else if child.has_tag("zone") && !opts.keep_zones {
                if passes_net(net_of(&child)) {
                    remove = match &layer_set {
                        Some(ls) => child
                            .find("layer")
                            .and_then(|l| gs(l, 0))
                            .is_some_and(|l| ls.contains(&l)),
                        None => true,
                    };
                }
                if remove && region.is_some() {
                    let pts: Vec<Point> = child
                        .find("polygon")
                        .and_then(|p| p.find("pts"))
                        .map(|pts| pts.find_all("xy").map(xy).collect())
                        .unwrap_or_default();
                    remove = !pts.is_empty() && pts.iter().all(|&(x, y)| in_region(x, y));
                }
                if remove {
                    stats.zones += 1;
                }
            }
            if !remove {
                kept.push(child);
            }
        }
        self.doc.root.children = kept;

        let q4 = |v: f64| quant(v, 4);
        let mut removed_orphans = 0;
        if opts.remove_orphan_vias && layer_set.is_some() {
            let mut endpoints: HashSet<(i64, i64, String)> = HashSet::new();
            for c in self
                .doc
                .root
                .children
                .iter()
                .filter(|c| c.has_tag("segment"))
            {
                let layer = c.find("layer").and_then(|l| gs(l, 0)).unwrap_or_default();
                for tag in ["start", "end"] {
                    if let Some(n) = c.find(tag) {
                        let (x, y) = (gf(n, 0).unwrap_or(0.0), gf(n, 1).unwrap_or(0.0));
                        endpoints.insert((q4(x), q4(y), layer.clone()));
                    }
                }
            }
            self.doc.root.children.retain(|c| {
                if !c.has_tag("via") {
                    return true;
                }
                let (Some(at), Some(l)) = (c.find("at"), c.find("layers")) else {
                    return true;
                };
                let (x, y) = xy(at);
                let connected = string_atoms(l)
                    .into_iter()
                    .any(|vl| endpoints.contains(&(q4(x), q4(y), vl)));
                if !connected {
                    removed_orphans += 1;
                }
                connected
            });
            stats.vias += removed_orphans;
        }

        // Keep the parsed view in lock-step with the tree.
        for seg in &mut self.segments {
            if let Some(&(s, e)) = clipped.get(&seg.uuid) {
                seg.start = s;
                seg.end = e;
            }
        }
        let has_filter =
            net_numbers.is_some() || layer_set.is_some() || opts.exclude_power || region.is_some();
        let inside = |p: Point| match region {
            Some((x1, y1, x2, y2)) => x1 <= p.0 && p.0 <= x2 && y1 <= p.1 && p.1 <= y2,
            None => true,
        };
        let kept_by_filters = |net: i64, layer_ok: bool| {
            (opts.exclude_power && power.contains(&net))
                || net_numbers.as_ref().is_some_and(|s| !s.contains(&net))
                || !layer_ok
        };
        if has_filter {
            self.segments.retain(|s| {
                let layer_ok = layer_set.as_ref().is_none_or(|ls| ls.contains(&s.layer));
                kept_by_filters(s.net_number, layer_ok)
                    || (region.is_some() && !(inside(s.start) && inside(s.end)))
            });
            self.vias.retain(|v| {
                let layer_ok = layer_set
                    .as_ref()
                    .is_none_or(|ls| v.layers.iter().all(|l| ls.contains(l)));
                kept_by_filters(v.net_number, layer_ok) || !inside(v.position)
            });
            if !opts.keep_zones {
                self.zones.retain(|z| {
                    let layer_ok = layer_set.as_ref().is_none_or(|ls| ls.contains(&z.layer));
                    kept_by_filters(z.net_number, layer_ok)
                        || (region.is_some()
                            && (z.polygon.is_empty() || !z.polygon.iter().all(|&p| inside(p))))
                });
            }
        } else {
            self.segments.clear();
            self.vias.clear();
            if !opts.keep_zones {
                self.zones.clear();
            }
        }
        if opts.remove_orphan_vias && layer_set.is_some() && removed_orphans > 0 {
            let eps: HashSet<(i64, i64, String)> = self
                .segments
                .iter()
                .flat_map(|s| {
                    [
                        (q4(s.start.0), q4(s.start.1), s.layer.clone()),
                        (q4(s.end.0), q4(s.end.1), s.layer.clone()),
                    ]
                })
                .collect();
            self.vias.retain(|v| {
                v.layers
                    .iter()
                    .any(|l| eps.contains(&(q4(v.position.0), q4(v.position.1), l.clone())))
            });
        }
        self.invalidate_dedup_keys();
        stats
    }
}
