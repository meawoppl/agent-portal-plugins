//! Port of `kicad_tools.router.optimizer.chain`: split segments into linear
//! chains at junctions (degree >= 3 vertices), issue #2389.

use std::collections::{HashMap, HashSet};
use super::geometry::SegmentExt;

use crate::router::primitives::Segment;

type Key = (i64, i64);

fn quantize(value: f64, tolerance: f64) -> i64 {
    if tolerance <= 0.0 {
        return (value * 1e9).round_ties_even() as i64;
    }
    (value / tolerance).round_ties_even() as i64
}

fn vertex_key(x: f64, y: f64, tolerance: f64) -> Key {
    (quantize(x, tolerance), quantize(y, tolerance))
}

/// Insertion-ordered vertex -> incident `(segment index, which end)` map.
struct Adjacency {
    order: Vec<Key>,
    map: HashMap<Key, Vec<(usize, u8)>>,
}

impl Adjacency {
    fn push(&mut self, key: Key, item: (usize, u8)) {
        self.map
            .entry(key)
            .or_insert_with(|| {
                self.order.push(key);
                Vec::new()
            })
            .push(item);
    }

    fn get(&self, key: &Key) -> &[(usize, u8)] {
        self.map.get(key).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Sort segments into connected linear chains (oriented head-to-tail).
pub fn sort_into_chains(segments: &[Segment], tolerance: f64) -> Vec<Vec<Segment>> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut adj = Adjacency {
        order: Vec::new(),
        map: HashMap::new(),
    };
    for (idx, seg) in segments.iter().enumerate() {
        adj.push(vertex_key(seg.x1, seg.y1, tolerance), (idx, 0));
        adj.push(vertex_key(seg.x2, seg.y2, tolerance), (idx, 1));
    }

    let mut visited: HashSet<usize> = HashSet::new();
    let mut chains = Vec::new();
    let boundary: Vec<Key> = adj
        .order
        .iter()
        .copied()
        .filter(|k| adj.get(k).len() != 2)
        .collect();
    for key in boundary {
        for &(seg_idx, which) in adj.get(&key).to_vec().iter() {
            if visited.contains(&seg_idx) {
                continue;
            }
            let chain = walk_linear_chain(segments, &adj, seg_idx, which, &mut visited, tolerance);
            if !chain.is_empty() {
                chains.push(chain);
            }
        }
    }
    for idx in 0..segments.len() {
        if visited.contains(&idx) {
            continue;
        }
        let chain = walk_linear_chain(segments, &adj, idx, 0, &mut visited, tolerance);
        if !chain.is_empty() {
            chains.push(chain);
        }
    }
    chains
}

fn walk_linear_chain(
    segments: &[Segment],
    adj: &Adjacency,
    start_idx: usize,
    start_which: u8,
    visited: &mut HashSet<usize>,
    tolerance: f64,
) -> Vec<Segment> {
    if visited.contains(&start_idx) {
        return Vec::new();
    }
    visited.insert(start_idx);
    let mut seg = segments[start_idx].clone();
    if start_which == 1 {
        seg = flip_segment(&seg);
    }
    let mut current = vertex_key(seg.x2, seg.y2, tolerance);
    let mut chain = vec![seg];
    loop {
        let incident = adj.get(&current);
        if incident.len() != 2 {
            break;
        }
        let Some(&(next_idx, next_which)) = incident.iter().find(|(i, _)| !visited.contains(i))
        else {
            break;
        };
        visited.insert(next_idx);
        let mut next = segments[next_idx].clone();
        if next_which == 1 {
            next = flip_segment(&next);
        }
        current = vertex_key(next.x2, next.y2, tolerance);
        chain.push(next);
    }
    chain
}

fn flip_segment(seg: &Segment) -> Segment {
    seg.with_points(seg.x2, seg.y2, seg.x1, seg.y1)
}

/// Legacy single-chain sorter: concatenation of [`sort_into_chains`].
pub fn sort_chain_segments(segments: &[Segment], tolerance: f64) -> Vec<Segment> {
    if segments.len() <= 1 {
        return segments.to_vec();
    }
    sort_into_chains(segments, tolerance)
        .into_iter()
        .flatten()
        .collect()
}
