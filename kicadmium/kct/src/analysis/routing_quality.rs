//! Advisory routing-quality metrics over PCB copper segments (port of
//! `kicad_tools.analysis.routing_quality`, issue #4623).

use std::collections::{HashMap, HashSet};

use crate::jobj;
use crate::pyjson::Json;
use crate::schema::pcb::Pcb;

pub const FRAGMENT_LENGTH_MM: f64 = 0.25;
pub const STAIRCASE_STEP_MM: f64 = 0.6;
pub const ANGLE_TOLERANCE_DEG: f64 = 0.5;
pub const COORD_EPSILON_MM: f64 = 1e-6;
pub const RULE_FRAGMENT_FRACTION: &str = "routing_quality_fragment_fraction";
pub const RULE_STAIRCASE_FRACTION: &str = "routing_quality_staircase_fraction";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoutingQualityMetrics {
    pub total_segments: i64,
    pub nets_with_copper: i64,
    pub segments_per_net: f64,
    pub zero_length_count: i64,
    pub median_length_mm: f64,
    pub fragment_count: i64,
    pub fragment_fraction: f64,
    pub orthogonal_count: i64,
    pub diagonal_45_count: i64,
    pub off_axis_count: i64,
    pub staircase_step_count: i64,
    pub staircase_fraction: f64,
    pub copper_arc_count: i64,
    pub copper_arc_length_mm: f64,
}

impl RoutingQualityMetrics {
    pub fn diagonal_45_fraction(&self) -> f64 {
        let nonzero = self.total_segments - self.zero_length_count;
        if nonzero > 0 {
            self.diagonal_45_count as f64 / nonzero as f64
        } else {
            0.0
        }
    }

    pub fn to_dict(&self) -> Json {
        let mut d = jobj! {
            "total_segments" => self.total_segments,
            "nets_with_copper" => self.nets_with_copper,
            "segments_per_net" => self.segments_per_net,
            "zero_length_count" => self.zero_length_count,
            "median_length_mm" => self.median_length_mm,
            "fragment_count" => self.fragment_count,
            "fragment_fraction" => self.fragment_fraction,
            "orthogonal_count" => self.orthogonal_count,
            "diagonal_45_count" => self.diagonal_45_count,
            "off_axis_count" => self.off_axis_count,
            "staircase_step_count" => self.staircase_step_count,
            "staircase_fraction" => self.staircase_fraction,
        };
        if self.copper_arc_count != 0 {
            d.set("copper_arc_count", self.copper_arc_count);
            d.set("copper_arc_length_mm", self.copper_arc_length_mm);
        }
        d
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdBreach {
    pub rule_id: &'static str,
    pub metric: &'static str,
    pub actual: f64,
    pub limit: f64,
    pub message: String,
}

pub fn evaluate_routing_quality_thresholds(
    m: &RoutingQualityMetrics,
    max_fragment_fraction: Option<f64>,
    max_staircase_fraction: Option<f64>,
) -> Vec<ThresholdBreach> {
    let mut out = Vec::new();
    if let Some(limit) = max_fragment_fraction {
        if m.fragment_fraction > limit {
            out.push(ThresholdBreach {
                rule_id: RULE_FRAGMENT_FRACTION,
                metric: "fragment_fraction",
                actual: m.fragment_fraction,
                limit,
                message: format!(
                    "Routing-quality gate: fragment_fraction {:.4} exceeds the \
                     --max-fragment-fraction ceiling {limit:.4} ({} segment(s) shorter than \
                     {FRAGMENT_LENGTH_MM} mm)",
                    m.fragment_fraction, m.fragment_count
                ),
            });
        }
    }
    if let Some(limit) = max_staircase_fraction {
        if m.staircase_fraction > limit {
            out.push(ThresholdBreach {
                rule_id: RULE_STAIRCASE_FRACTION,
                metric: "staircase_fraction",
                actual: m.staircase_fraction,
                limit,
                message: format!(
                    "Routing-quality gate: staircase_fraction {:.4} exceeds the \
                     --max-staircase-fraction ceiling {limit:.4} ({} H/V staircase step(s) with \
                     legs shorter than {STAIRCASE_STEP_MM} mm)",
                    m.staircase_fraction, m.staircase_step_count
                ),
            });
        }
    }
    out
}

pub fn routing_quality_gate_dict(
    breaches: &[ThresholdBreach],
    max_fragment_fraction: Option<f64>,
    max_staircase_fraction: Option<f64>,
) -> Json {
    jobj! {
        "thresholds" => jobj! {
            "max_fragment_fraction" => max_fragment_fraction,
            "max_staircase_fraction" => max_staircase_fraction,
        },
        "gate_passed" => breaches.is_empty(),
        "gate_breaches" => Json::Arr(breaches.iter().map(|b| jobj! {
            "rule_id" => b.rule_id,
            "metric" => b.metric,
            "actual" => b.actual,
            "limit" => b.limit,
        }).collect()),
    }
}

fn quantize(p: (f64, f64)) -> (i64, i64) {
    (
        (p.0 / COORD_EPSILON_MM).round_ties_even() as i64,
        (p.1 / COORD_EPSILON_MM).round_ties_even() as i64,
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Dir {
    Horizontal,
    Vertical,
    Diagonal45,
    OffAxis,
}

fn classify_direction(dx: f64, dy: f64) -> Dir {
    let angle = dy.abs().atan2(dx.abs()).to_degrees();
    if angle <= ANGLE_TOLERANCE_DEG {
        Dir::Horizontal
    } else if angle >= 90.0 - ANGLE_TOLERANCE_DEG {
        Dir::Vertical
    } else if (angle - 45.0).abs() <= ANGLE_TOLERANCE_DEG {
        Dir::Diagonal45
    } else {
        Dir::OffAxis
    }
}

/// `(start, end, layer, net_number)` record.
pub type QualitySegmentRecord = ((f64, f64), (f64, f64), String, i64);

/// Python `statistics.median`.
pub fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

pub fn compute_routing_quality_from_records(
    records: &[QualitySegmentRecord],
) -> RoutingQualityMetrics {
    type EndpointKey<'a> = (i64, &'a str, (i64, i64));
    type ShortSegment<'a> = (Dir, i64, &'a str, (i64, i64), (i64, i64));
    let total_segments = records.len() as i64;
    let nets: HashSet<i64> = records.iter().map(|r| r.3).filter(|n| *n != 0).collect();
    let nets_with_copper = nets.len() as i64;
    let segments_per_net = if nets_with_copper > 0 {
        total_segments as f64 / nets_with_copper as f64
    } else {
        0.0
    };
    let mut m = RoutingQualityMetrics {
        total_segments,
        nets_with_copper,
        segments_per_net,
        ..Default::default()
    };
    let mut lengths = Vec::new();
    let mut endpoints: HashMap<EndpointKey<'_>, HashSet<Dir>> = HashMap::new();
    let mut shorts: Vec<ShortSegment<'_>> = Vec::new();
    for (start, end, layer, net) in records {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let length = dx.hypot(dy);
        if length <= COORD_EPSILON_MM {
            m.zero_length_count += 1;
            continue;
        }
        lengths.push(length);
        if length < FRAGMENT_LENGTH_MM {
            m.fragment_count += 1;
        }
        match classify_direction(dx, dy) {
            d @ (Dir::Horizontal | Dir::Vertical) => {
                m.orthogonal_count += 1;
                if length < STAIRCASE_STEP_MM {
                    let p1 = quantize(*start);
                    let p2 = quantize(*end);
                    shorts.push((d, *net, layer.as_str(), p1, p2));
                    for p in [p1, p2] {
                        endpoints
                            .entry((*net, layer.as_str(), p))
                            .or_default()
                            .insert(d);
                    }
                }
            }
            Dir::Diagonal45 => m.diagonal_45_count += 1,
            Dir::OffAxis => m.off_axis_count += 1,
        }
    }
    for (d, net, layer, p1, p2) in &shorts {
        let perp = if *d == Dir::Horizontal {
            Dir::Vertical
        } else {
            Dir::Horizontal
        };
        for p in [p1, p2] {
            if endpoints
                .get(&(*net, *layer, *p))
                .is_some_and(|s| s.contains(&perp))
            {
                m.staircase_step_count += 1;
                break;
            }
        }
    }
    let nonzero = lengths.len();
    m.median_length_mm = if lengths.is_empty() {
        0.0
    } else {
        median(&lengths)
    };
    if nonzero > 0 {
        m.fragment_fraction = m.fragment_count as f64 / nonzero as f64;
        m.staircase_fraction = m.staircase_step_count as f64 / nonzero as f64;
    }
    m
}

/// `compute_routing_quality(pcb)`.
pub fn compute_routing_quality(pcb: &Pcb) -> RoutingQualityMetrics {
    let records: Vec<QualitySegmentRecord> = pcb
        .segments()
        .iter()
        .map(|s| (s.start, s.end, s.layer.clone(), s.net_number))
        .collect();
    let mut m = compute_routing_quality_from_records(&records);
    m.copper_arc_count = pcb.arcs().len() as i64;
    m.copper_arc_length_mm = pcb
        .arcs()
        .iter()
        .map(|a| a.length())
        .fold(0.0, |a, b| a + b);
    m
}
