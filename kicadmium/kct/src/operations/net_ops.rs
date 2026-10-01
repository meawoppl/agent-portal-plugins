//! Port of `kicad_tools.operations.net_ops`: trace electrical connections
//! through a schematic by following wires between pins, junctions and labels.
//!
//! Like upstream, pins are modelled at their symbol's position (library pin
//! offsets are not resolved), and unlabelled nets get a
//! `Net_XXXX` name derived from CPython's `hash()` of the visited points.

use std::collections::{BTreeSet, HashMap};

use crate::pyjson::py_round;
use crate::schema::schematic::Schematic;
use crate::schema::wire::Wire;

/// Tolerance for point matching (mm).
pub const POINT_TOLERANCE: f64 = 0.1;

pub fn points_equal(p1: (f64, f64), p2: (f64, f64), tol: f64) -> bool {
    (p1.0 - p2.0).abs() < tol && (p1.1 - p2.1).abs() < tol
}

pub fn point_on_wire(point: (f64, f64), wire: &Wire, tol: f64) -> bool {
    wire.contains_point(point, tol)
}

/// A connection point on a net.
#[derive(Debug, Clone, PartialEq)]
pub struct NetConnection {
    pub point: (f64, f64),
    /// `"pin"`, `"wire_end"`, `"junction"` or `"label"`.
    pub kind: String,
    /// Symbol reference or label text.
    pub reference: String,
    pub pin_number: String,
    pub uuid: String,
}

/// A traced net with all its connections.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Net {
    pub name: String,
    pub connections: Vec<NetConnection>,
    pub wires: Vec<Wire>,
    pub has_label: bool,
}

impl Net {
    pub fn pin_count(&self) -> usize {
        self.connections.iter().filter(|c| c.kind == "pin").count()
    }

    pub fn symbol_refs(&self) -> BTreeSet<String> {
        self.connections
            .iter()
            .filter(|c| c.kind == "pin" && !c.reference.is_empty())
            .map(|c| c.reference.clone())
            .collect()
    }

    /// Python `repr(net)`.
    pub fn repr(&self) -> String {
        format!(
            "Net({}, {} pins, {} wires)",
            crate::pyjson::py_repr_str(&self.name),
            self.pin_count(),
            self.wires.len()
        )
    }
}

type Key = (u64, u64);

fn key_of(p: (f64, f64)) -> ((f64, f64), Key) {
    let k = (py_round(p.0, 1), py_round(p.1, 1));
    // Normalise -0.0 so it matches 0.0 like Python dict keys do.
    let bits = |v: f64| {
        if v == 0.0 {
            0.0f64.to_bits()
        } else {
            v.to_bits()
        }
    };
    (k, (bits(k.0), bits(k.1)))
}

/// CPython `hash(float)` (`_Py_HashDouble`) for finite values.
pub fn py_hash_float(v: f64) -> i64 {
    const BITS: i32 = 61;
    const MODULUS: u64 = (1 << 61) - 1;
    if v.is_nan() {
        return 0;
    }
    if v.is_infinite() {
        return if v > 0.0 { 314159 } else { -314159 };
    }
    if v == 0.0 {
        return 0;
    }
    let (mut m, mut e) = frexp(v);
    let sign: i64 = if m < 0.0 {
        m = -m;
        -1
    } else {
        1
    };
    let mut x: u64 = 0;
    while m != 0.0 {
        x = ((x << 28) & MODULUS) | (x >> (BITS - 28));
        m *= 268_435_456.0;
        e -= 28;
        let y = m as u64;
        m -= y as f64;
        x += y;
        if x >= MODULUS {
            x -= MODULUS;
        }
    }
    let e = if e >= 0 {
        e % BITS
    } else {
        BITS - 1 - ((-1 - e) % BITS)
    };
    x = ((x << e) & MODULUS) | (x >> (BITS - e));
    let h = (x as i64) * sign;
    if h == -1 {
        -2
    } else {
        h
    }
}

fn frexp(v: f64) -> (f64, i32) {
    if v == 0.0 || !v.is_finite() {
        return (v, 0);
    }
    let bits = v.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // Subnormal: scale up first.
        let (m, e) = frexp(v * f64::from_bits(0x4350_0000_0000_0000)); // 2^54
        return (m, e - 54);
    }
    let e = exp - 1022;
    let m = f64::from_bits((bits & !(0x7ff << 52)) | (1022u64 << 52));
    (m, e)
}

/// CPython tuple hash over already-hashed lanes.
pub fn py_hash_tuple(lanes: &[i64]) -> i64 {
    const P1: u64 = 11400714785074694791;
    const P2: u64 = 14029467366897019727;
    const P5: u64 = 2870177450012600261;
    let mut acc: u64 = P5;
    for &lane in lanes {
        acc = acc.wrapping_add((lane as u64).wrapping_mul(P2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(P1);
    }
    acc = acc.wrapping_add((lanes.len() as u64) ^ (P5 ^ 3527539));
    if acc == u64::MAX {
        return 1546275796;
    }
    acc as i64
}

/// Traces nets through a schematic.
pub struct NetTracer<'a> {
    pub sch: &'a Schematic,
    /// Symbol pins keyed by rounded symbol position.
    pub pin_positions: HashMap<Key, Vec<NetConnection>>,
    wire_endpoints: HashMap<Key, Vec<usize>>,
}

impl<'a> NetTracer<'a> {
    pub fn new(sch: &'a Schematic) -> Self {
        let mut pin_positions: HashMap<Key, Vec<NetConnection>> = HashMap::new();
        for sym in sch.symbols() {
            for pin in &sym.pins {
                let (_, k) = key_of(sym.position);
                pin_positions.entry(k).or_default().push(NetConnection {
                    point: sym.position,
                    kind: "pin".into(),
                    reference: sym.reference().to_string(),
                    pin_number: pin.number.clone(),
                    uuid: pin.uuid.clone(),
                });
            }
        }
        let mut wire_endpoints: HashMap<Key, Vec<usize>> = HashMap::new();
        for (i, w) in sch.wires().iter().enumerate() {
            for p in [w.start, w.end] {
                wire_endpoints.entry(key_of(p).1).or_default().push(i);
            }
        }
        NetTracer {
            sch,
            pin_positions,
            wire_endpoints,
        }
    }

    /// Label text at or near a point (local, then global, then hierarchical).
    pub fn get_label_at(&self, point: (f64, f64)) -> Option<String> {
        let t = POINT_TOLERANCE;
        if let Some(l) = self
            .sch
            .labels()
            .iter()
            .find(|l| points_equal(l.position, point, t))
        {
            return Some(l.text.clone());
        }
        if let Some(l) = self
            .sch
            .global_labels()
            .into_iter()
            .find(|l| points_equal(l.position, point, t))
        {
            return Some(l.text);
        }
        self.sch
            .hierarchical_labels()
            .iter()
            .find(|l| points_equal(l.position, point, t))
            .map(|l| l.text.clone())
    }

    /// Trace a net from `start`, following wires through junctions.
    /// `visited_wires` holds wire UUIDs already assigned to a net.
    pub fn trace_from_point(&self, start: (f64, f64), visited_wires: &mut BTreeSet<String>) -> Net {
        let mut net = Net::default();
        let mut to_visit = vec![start];
        let mut visited: Vec<(f64, f64)> = Vec::new();
        let mut visited_keys: BTreeSet<Key> = BTreeSet::new();
        let wires = self.sch.wires();
        while let Some(point) = to_visit.pop() {
            let (kf, k) = key_of(point);
            if !visited_keys.insert(k) {
                continue;
            }
            visited.push(kf);
            if let Some(label) = self.get_label_at(point).filter(|l| !l.is_empty()) {
                net.name = label.clone();
                net.has_label = true;
                net.connections.push(NetConnection {
                    point,
                    kind: "label".into(),
                    reference: label,
                    pin_number: String::new(),
                    uuid: String::new(),
                });
            }
            if let Some(j) = self
                .sch
                .junctions()
                .iter()
                .find(|j| points_equal(j.position, point, POINT_TOLERANCE))
            {
                net.connections.push(NetConnection {
                    point,
                    kind: "junction".into(),
                    reference: String::new(),
                    pin_number: String::new(),
                    uuid: j.uuid.clone(),
                });
            }
            if let Some(ids) = self.wire_endpoints.get(&k) {
                for &wi in ids {
                    let w = &wires[wi];
                    if visited_wires.contains(&w.uuid) {
                        continue;
                    }
                    visited_wires.insert(w.uuid.clone());
                    net.wires.push(w.clone());
                    if points_equal(w.start, point, POINT_TOLERANCE) {
                        to_visit.push(w.end);
                    } else {
                        to_visit.push(w.start);
                    }
                }
            }
        }
        if net.name.is_empty() {
            let mut pts = visited;
            pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let lanes: Vec<i64> = pts
                .iter()
                .map(|p| py_hash_tuple(&[py_hash_float(p.0), py_hash_float(p.1)]))
                .collect();
            let h = py_hash_tuple(&lanes) & 0xFFFF;
            net.name = format!("Net_{h:04X}");
        }
        net
    }

    /// Trace all nets (every unvisited wire, then unconnected labels).
    pub fn trace_all_nets(&self) -> Vec<Net> {
        let mut nets: Vec<Net> = Vec::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        for w in self.sch.wires() {
            if !visited.contains(&w.uuid) {
                let net = self.trace_from_point(w.start, &mut visited);
                if !net.wires.is_empty() {
                    nets.push(net);
                }
            }
        }
        for lbl in self.sch.labels() {
            let traced = nets.iter().any(|n| {
                n.connections
                    .iter()
                    .any(|c| points_equal(lbl.position, c.point, POINT_TOLERANCE))
            });
            if !traced {
                nets.push(Net {
                    name: lbl.text.clone(),
                    has_label: true,
                    connections: vec![NetConnection {
                        point: lbl.position,
                        kind: "label".into(),
                        reference: lbl.text.clone(),
                        pin_number: String::new(),
                        uuid: lbl.uuid.clone(),
                    }],
                    wires: Vec::new(),
                });
            }
        }
        nets
    }

    /// Find a net by label text (the last matching label kind wins, as
    /// upstream's sequential loops do).
    pub fn find_net_by_label(&self, label_text: &str) -> Option<Net> {
        let mut pos = None;
        if let Some(l) = self.sch.labels().iter().find(|l| l.text == label_text) {
            pos = Some(l.position);
        }
        if let Some(l) = self
            .sch
            .global_labels()
            .into_iter()
            .find(|l| l.text == label_text)
        {
            pos = Some(l.position);
        }
        if let Some(l) = self
            .sch
            .hierarchical_labels()
            .iter()
            .find(|l| l.text == label_text)
        {
            pos = Some(l.position);
        }
        pos.map(|p| self.trace_from_point(p, &mut BTreeSet::new()))
    }
}

/// Trace all nets in a schematic.
pub fn trace_nets(sch: &Schematic) -> Vec<Net> {
    NetTracer::new(sch).trace_all_nets()
}

/// Find a net by label.
pub fn find_net(sch: &Schematic, label: &str) -> Option<Net> {
    NetTracer::new(sch).find_net_by_label(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_hash_matches_cpython() {
        // Values from CPython 3.12: hash(1.5), hash(-2.25), hash(0.1), hash(100.0)
        assert_eq!(py_hash_float(1.5), 1152921504606846977);
        assert_eq!(py_hash_float(-2.25), -576460752303423490);
        assert_eq!(py_hash_float(0.1), 230584300921369408);
        assert_eq!(py_hash_float(100.0), 100);
    }
}
