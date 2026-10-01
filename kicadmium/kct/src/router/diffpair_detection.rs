//! Layered differential-pair detection (port of
//! `kicad_tools.router.diffpair_detection`, issue #2558): explicit
//! `diffpair_partner` declarations, then KiCad `diff_pair_template` groups,
//! then suffix inference.

use super::diffpair::{
    detect_differential_pairs, detect_pair_type, parse_differential_signal, DifferentialPair,
    DifferentialSignal,
};
use super::rules::{NetClassMap, NetClassRouting};
use crate::sexp::SExp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionSource {
    Explicit,
    KicadGroup,
    Suffix,
}

impl DetectionSource {
    pub fn value(self) -> &'static str {
        match self {
            DetectionSource::Explicit => "explicit",
            DetectionSource::KicadGroup => "kicad_group",
            DetectionSource::Suffix => "suffix",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectedPair {
    pub pair: DifferentialPair,
    pub source: DetectionSource,
}

/// The `synth_routing` / `net_to_class` idiom: a `{key: NetClassRouting}`
/// view whose values are indices into a `{net_name: NetClassRouting}` map
/// (index equality stands in for Python object identity), keyed by net
/// name and, via `setdefault`, by class name.
#[derive(Debug, Clone, Default)]
pub struct SynthRouting<'a> {
    pub map: &'a [(String, NetClassRouting)],
    pub keys: Vec<(String, usize)>,
    pub net_to_class: Vec<(String, String)>,
}

impl<'a> SynthRouting<'a> {
    /// `synth = dict(net_class_map); synth.setdefault(nc.name, nc)` plus
    /// `net_to_class[net] = nc.name`.
    pub fn from_net_class_map(map: &'a NetClassMap) -> Self {
        let mut keys: Vec<(String, usize)> = map
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i))
            .collect();
        let mut net_to_class = Vec::new();
        for (i, (net, nc)) in map.iter().enumerate() {
            net_to_class.push((net.clone(), nc.name.clone()));
            if !keys.iter().any(|(k, _)| *k == nc.name) {
                keys.push((nc.name.clone(), i));
            }
        }
        SynthRouting {
            map,
            keys,
            net_to_class,
        }
    }

    pub fn get(&self, key: &str) -> Option<(usize, &'a NetClassRouting)> {
        self.keys
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, i)| (*i, &self.map[*i].1))
    }

    pub fn class_of(&self, net: &str) -> Option<&str> {
        self.net_to_class
            .iter()
            .find(|(n, _)| n == net)
            .map(|(_, c)| c.as_str())
    }

    /// `_lookup_net_class`: class-name-keyed via `net_to_class` first, then
    /// net-name-keyed.
    pub fn lookup_net_class(&self, net: &str) -> Option<&'a NetClassRouting> {
        if let Some(c) = self.class_of(net) {
            if let Some((_, nc)) = self.get(c) {
                return Some(nc);
            }
        }
        self.get(net).map(|(_, nc)| nc)
    }
}

fn name_to_id(net_names: &[(i64, String)], name: &str) -> Option<i64> {
    net_names.iter().find(|(_, n)| n == name).map(|(id, _)| *id)
}

fn common_prefix(a: &str, b: &str) -> String {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut i = 0;
    while i < ac.len().min(bc.len()) && ac[i] == bc[i] {
        i += 1;
    }
    let prefix: String = ac[..i].iter().collect();
    prefix.trim_end_matches(['_', '-', '+']).to_string()
}

fn make_pair_from_names(
    p_name: &str,
    n_name: &str,
    net_names: &[(i64, String)],
) -> Option<DifferentialPair> {
    let p_id = name_to_id(net_names, p_name)?;
    let n_id = name_to_id(net_names, n_name)?;
    let cp = common_prefix(p_name, n_name);
    let base = if cp.is_empty() {
        p_name.to_string()
    } else {
        cp
    };
    let sig = |name: &str, id: i64, pol: &'static str| DifferentialSignal {
        net_name: name.to_string(),
        net_id: id,
        base_name: base.clone(),
        polarity: pol,
        notation: "explicit",
    };
    Some(DifferentialPair {
        name: base.clone(),
        positive: sig(p_name, p_id, "P"),
        negative: sig(n_name, n_id, "N"),
        pair_type: detect_pair_type(&base),
    })
}

fn is_polarity_counterpart(a: &str, b: &str) -> bool {
    match (parse_differential_signal(a), parse_differential_signal(b)) {
        (Some(x), Some(y)) => x.0 == y.0 && x.1 != y.1,
        _ => false,
    }
}

fn order_explicit_pair(a: &str, b: &str) -> (String, String) {
    let ap = parse_differential_signal(a);
    let bp = parse_differential_signal(b);
    let (a, b) = (a.to_string(), b.to_string());
    match (ap.as_ref().map(|x| x.1), bp.as_ref().map(|x| x.1)) {
        (Some("P"), _) => (a, b),
        (Some("N"), _) => (b, a),
        (_, Some("P")) => (b, a),
        (_, Some("N")) => (a, b),
        _ => {
            if a <= b {
                (a, b)
            } else {
                (b, a)
            }
        }
    }
}

fn declared_partner_for_net(
    net: &str,
    routing: &SynthRouting,
    class_members: &[(String, Vec<String>)],
) -> Option<String> {
    let own = routing.get(net);
    if let Some((_, nc)) = own {
        if let Some(p) = nc.diffpair_partner.as_deref().filter(|p| !p.is_empty()) {
            return (p != net).then(|| p.to_string());
        }
    }
    let class = routing.class_of(net)?;
    if class == net {
        return None;
    }
    let (ci, cnc) = routing.get(class)?;
    if own.is_some_and(|(i, _)| i == ci) {
        return None;
    }
    let partner = cnc.diffpair_partner.as_deref().filter(|p| !p.is_empty())?;
    if partner == net {
        return None;
    }
    let members: &[String] = class_members
        .iter()
        .find(|(c, _)| c == class)
        .map(|(_, m)| m.as_slice())
        .unwrap_or(&[]);
    let candidates: Vec<&String> = members.iter().filter(|m| *m != partner).collect();
    if candidates.len() <= 1 {
        return Some(partner.to_string());
    }
    if !is_polarity_counterpart(net, partner) {
        return None;
    }
    let rivals = candidates
        .iter()
        .any(|m| *m != net && is_polarity_counterpart(m, partner));
    (!rivals).then(|| partner.to_string())
}

fn gather_explicit_pairs(
    net_names: &[(i64, String)],
    routing: Option<&SynthRouting>,
) -> Vec<DifferentialPair> {
    let Some(routing) = routing.filter(|r| !r.keys.is_empty() && !r.net_to_class.is_empty()) else {
        return vec![];
    };
    let mut class_members: Vec<(String, Vec<String>)> = Vec::new();
    for (_, net) in net_names {
        if let Some(c) = routing.class_of(net) {
            let i = match class_members.iter().position(|(k, _)| k == c) {
                Some(i) => i,
                None => {
                    class_members.push((c.to_string(), vec![]));
                    class_members.len() - 1
                }
            };
            if !class_members[i].1.contains(net) {
                class_members[i].1.push(net.clone());
            }
        }
    }
    let mut declared: Vec<(String, String)> = Vec::new();
    for (_, net) in net_names {
        if let Some(p) = declared_partner_for_net(net, routing, &class_members) {
            match declared.iter_mut().find(|(n, _)| n == net) {
                Some(e) => e.1 = p,
                None => declared.push((net.clone(), p)),
            }
        }
    }
    let mut pairs = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for (net, partner) in &declared {
        let key = if net <= partner {
            (net.clone(), partner.clone())
        } else {
            (partner.clone(), net.clone())
        };
        if seen.contains(&key) {
            continue;
        }
        if name_to_id(net_names, partner).is_none() {
            continue;
        }
        if used.contains(net) || used.contains(partner) {
            continue;
        }
        seen.push(key);
        let (pos, neg) = order_explicit_pair(net, partner);
        if let Some(pair) = make_pair_from_names(&pos, &neg, net_names) {
            pairs.push(pair);
            used.push(net.clone());
            used.push(partner.clone());
        }
    }
    pairs
}

/// `detect_diff_pairs(net_names, net_class_routing=..., net_to_class=...,
/// kicad_groups=...)`; `net_names` is the ordered `{net_id: name}` dict.
pub fn detect_diff_pairs(
    net_names: &[(i64, String)],
    routing: Option<&SynthRouting>,
    kicad_groups: Option<&[(String, String)]>,
) -> Vec<DetectedPair> {
    let mut paired: Vec<i64> = Vec::new();
    let mut out = Vec::new();
    for pair in gather_explicit_pairs(net_names, routing) {
        paired.push(pair.positive.net_id);
        paired.push(pair.negative.net_id);
        out.push(DetectedPair {
            pair,
            source: DetectionSource::Explicit,
        });
    }
    for (p, n) in kicad_groups.unwrap_or(&[]) {
        let Some(pair) = make_pair_from_names(p, n, net_names) else {
            continue;
        };
        if paired.contains(&pair.positive.net_id) || paired.contains(&pair.negative.net_id) {
            continue;
        }
        paired.push(pair.positive.net_id);
        paired.push(pair.negative.net_id);
        out.push(DetectedPair {
            pair,
            source: DetectionSource::KicadGroup,
        });
    }
    let remaining: Vec<(i64, String)> = net_names
        .iter()
        .filter(|(id, _)| !paired.contains(id))
        .cloned()
        .collect();
    for pair in detect_differential_pairs(&remaining) {
        if paired.contains(&pair.positive.net_id) || paired.contains(&pair.negative.net_id) {
            continue;
        }
        paired.push(pair.positive.net_id);
        paired.push(pair.negative.net_id);
        out.push(DetectedPair {
            pair,
            source: DetectionSource::Suffix,
        });
    }
    out
}

/// `parse_diff_pair_templates_from_pcb`: `(diff_pair_template (positive
/// "A") (negative "B"))` direct children of the board root.
pub fn parse_diff_pair_templates_from_pcb(root: &SExp) -> Vec<(String, String)> {
    let first = |node: &SExp, name: &str| -> Option<String> {
        node.children
            .iter()
            .filter(|c| c.has_tag(name))
            .flat_map(|c| c.children.iter())
            .find_map(|sub| sub.value.as_ref().map(|v| v.to_string()))
    };
    root.children
        .iter()
        .filter(|c| c.has_tag("diff_pair_template"))
        .filter_map(|c| match (first(c, "positive"), first(c, "negative")) {
            (Some(p), Some(n)) if !p.is_empty() && !n.is_empty() => Some((p, n)),
            _ => None,
        })
        .collect()
}
