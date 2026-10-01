//! GEOS `TemplateSTRtree` replica (shapely `STRtree`, node capacity 10).
//!
//! Upstream emits violations in `STRtree.query` order, which is the tree's
//! depth-first traversal order, so the build (sort-tile-recursive with
//! x-slices then y-sorted nodes) is reproduced here. GEOS uses unstable
//! sorts; ties in envelope centres may order differently.

use super::shapely::Bounds;

const NODE_CAPACITY: usize = 10;

#[derive(Debug, Clone)]
struct Node {
    bounds: Bounds,
    /// Leaf: item index. Inner: child range in `nodes`.
    item: Option<usize>,
    children: (usize, usize),
}

#[derive(Debug, Clone, Default)]
pub struct StrTree {
    nodes: Vec<Node>,
    root: Option<usize>,
}

fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn intersects(a: &Bounds, b: &Bounds) -> bool {
    !(a.0 > b.2 || a.2 < b.0 || a.1 > b.3 || a.3 < b.1)
}

impl StrTree {
    /// Build over item envelopes (`None` = empty geometry, skipped).
    pub fn new(envelopes: &[Option<Bounds>]) -> Self {
        let mut nodes: Vec<Node> = envelopes
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                b.map(|bounds| Node {
                    bounds,
                    item: Some(i),
                    children: (0, 0),
                })
            })
            .collect();
        if nodes.is_empty() {
            return StrTree::default();
        }
        let mut begin = 0;
        let mut number = nodes.len();
        while number > 1 {
            let before = nodes.len();
            Self::create_parents(&mut nodes, begin, number);
            begin += number;
            number = nodes.len() - before;
        }
        let root = nodes.len() - 1;
        StrTree {
            nodes,
            root: Some(root),
        }
    }

    fn create_parents(nodes: &mut Vec<Node>, begin: usize, number: usize) {
        let min_leaf = (number as f64 / NODE_CAPACITY as f64).ceil();
        let slices = min_leaf.sqrt().ceil() as usize;
        let per_slice = (number as f64 / slices as f64).ceil() as usize;
        let end = begin + number;
        let cx = |n: &Node| n.bounds.0 + n.bounds.2;
        let cy = |n: &Node| n.bounds.1 + n.bounds.3;
        crate::utils::stdsort::std_sort(&mut nodes[begin..end], &|a: &Node, b: &Node| cx(a) < cx(b));
        let mut start = begin;
        for _ in 0..slices {
            if start >= end {
                break;
            }
            let stop = (start + per_slice).min(end);
            crate::utils::stdsort::std_sort(&mut nodes[start..stop], &|a: &Node, b: &Node| cy(a) < cy(b));
            let mut first = start;
            while first < stop {
                let last = (first + NODE_CAPACITY).min(stop);
                let mut b = nodes[first].bounds;
                for n in &nodes[first + 1..last] {
                    b = union(b, n.bounds);
                }
                nodes.push(Node {
                    bounds: b,
                    item: None,
                    children: (first, last),
                });
                first = last;
            }
            start = stop;
        }
    }

    /// Item indices whose envelopes intersect `q`, in GEOS traversal order.
    pub fn query(&self, q: Bounds) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(root) = self.root else {
            return out;
        };
        let r = &self.nodes[root];
        if let Some(item) = r.item {
            if intersects(&r.bounds, &q) {
                out.push(item);
            }
            return out;
        }
        self.visit(root, &q, &mut out);
        out
    }

    fn visit(&self, node: usize, q: &Bounds, out: &mut Vec<usize>) {
        let (a, b) = self.nodes[node].children;
        for c in a..b {
            let child = &self.nodes[c];
            if intersects(&child.bounds, q) {
                match child.item {
                    Some(i) => out.push(i),
                    None => self.visit(c, q, out),
                }
            }
        }
    }
}
