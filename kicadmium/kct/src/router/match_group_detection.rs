//! Layered match-group detection (port of
//! `kicad_tools.router.match_group_detection` plus the `MatchGroup` record
//! from `router.match_group_length`): explicit `length_match_group`
//! declarations, optional legacy groups, optional bus-suffix inference.

use std::sync::LazyLock;

use regex::Regex;

use super::diffpair::parse_differential_signal;
use super::diffpair_detection::SynthRouting;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchGroupSource {
    Explicit,
    LegacyApi,
    Suffix,
}

/// `MatchGroup`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchGroup {
    pub name: String,
    pub net_ids: Vec<i64>,
    pub reference_net_id: Option<i64>,
    pub source: MatchGroupSource,
    /// `(p_net_id, n_net_id)` diff-pair members.
    pub pair_ids: Vec<(i64, i64)>,
}

const MIN_GROUP_SIZE: usize = 3;

static BUS_GROUP_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)^DQ\d+$", "DDR_DATA"),
        (r"(?i)^DQS(?:\d+)?(?:_[PN])?$", "DDR_STROBE"),
        (r"(?i)^DM\d+$", "DDR_DATA_MASK"),
        (r"(?i)^CSI_DAT\d+_[PN]$", "MIPI_CSI_DATA"),
        (r"(?i)^DSI_DAT\d+_[PN]$", "MIPI_DSI_DATA"),
        (r"(?i)^TMDS_D\d+_[PN]$", "HDMI_TMDS_DATA"),
        (r"(?i)^A\d+$", "ADDR_BUS"),
    ]
    .into_iter()
    .map(|(p, n)| (Regex::new(p).unwrap(), n))
    .collect()
});

/// `CRITICAL_NET_PATTERNS[0:4]` (the clock discriminators).
static CLOCK_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"(?i)^CLK", r"(?i)CLK$", r"(?i)CLOCK", r"(?i)_CLK_"]
        .into_iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
});

fn name_of(net_names: &[(i64, String)], id: i64) -> Option<&str> {
    // `{net_id: name}` dict: the last entry per id wins.
    net_names.iter().rev().find(|(i, _)| *i == id).map(|(_, n)| n.as_str())
}

fn gather_explicit_groups(net_names: &[(i64, String)], routing: Option<&SynthRouting>) -> Vec<MatchGroup> {
    let Some(r) = routing.filter(|r| !r.keys.is_empty() && !r.net_to_class.is_empty()) else {
        return vec![];
    };
    let mut groups: Vec<(String, Vec<i64>)> = Vec::new();
    for (id, name) in net_names {
        let Some(class) = r.class_of(name) else {
            continue;
        };
        let Some((_, nc)) = r.get(class) else {
            continue;
        };
        let Some(g) = nc.length_match_group.as_ref() else {
            continue;
        };
        match groups.iter_mut().find(|(k, _)| k == g) {
            Some(e) => e.1.push(*id),
            None => groups.push((g.clone(), vec![*id])),
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    groups
        .into_iter()
        .map(|(name, mut ids)| {
            ids.sort();
            MatchGroup {
                name,
                net_ids: ids,
                reference_net_id: None,
                source: MatchGroupSource::Explicit,
                pair_ids: vec![],
            }
        })
        .collect()
}

fn infer_suffix_groups(net_names: &[(i64, String)]) -> Vec<MatchGroup> {
    let mut by: Vec<(&'static str, Vec<i64>)> = Vec::new();
    for (id, name) in net_names {
        for (re, g) in BUS_GROUP_PATTERNS.iter() {
            if re.is_match(name) {
                match by.iter_mut().find(|(k, _)| k == g) {
                    Some(e) => e.1.push(*id),
                    None => by.push((g, vec![*id])),
                }
                break;
            }
        }
    }
    by.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(&b.1)));
    by.into_iter()
        .filter(|(_, ids)| ids.len() >= MIN_GROUP_SIZE)
        .map(|(g, mut ids)| {
            ids.sort();
            MatchGroup {
                name: g.to_string(),
                net_ids: ids,
                reference_net_id: None,
                source: MatchGroupSource::Suffix,
                pair_ids: vec![],
            }
        })
        .collect()
}

fn resolve_reference(group: &MatchGroup, net_names: &[(i64, String)], routing: Option<&SynthRouting>) -> Option<i64> {
    let r = routing.filter(|r| !r.keys.is_empty() && !r.net_to_class.is_empty())?;
    let mut keys: Vec<&String> = r.keys.iter().map(|(k, _)| k).collect();
    keys.sort();
    let mut policy: Option<String> = None;
    for k in keys {
        let (_, nc) = r.get(k)?;
        if nc.length_match_group.as_deref() == Some(group.name.as_str()) {
            policy = nc.length_match_reference.clone();
            if policy.is_some() {
                break;
            }
        }
    }
    let policy = policy?;
    if policy != "clock" {
        let id = net_names.iter().find(|(_, n)| *n == policy).map(|(i, _)| *i)?;
        return group.net_ids.contains(&id).then_some(id);
    }
    let mut matches: Vec<i64> = group
        .net_ids
        .iter()
        .copied()
        .filter(|id| name_of(net_names, *id).is_some_and(|n| CLOCK_REGEXES.iter().any(|re| re.is_match(n))))
        .collect();
    matches.sort();
    matches.first().copied()
}

fn extract_pair_ids(group: &mut MatchGroup, net_names: &[(i64, String)]) {
    let mut by_key: Vec<((String, &'static str), Vec<(&'static str, i64)>)> = Vec::new();
    let mut new_ids: Vec<i64> = Vec::new();
    for &id in &group.net_ids {
        let Some(parsed) = name_of(net_names, id).and_then(parse_differential_signal) else {
            new_ids.push(id);
            continue;
        };
        let (base, pol, notation) = parsed;
        let k = (base, notation);
        let i = match by_key.iter().position(|(x, _)| *x == k) {
            Some(i) => i,
            None => {
                by_key.push((k, vec![]));
                by_key.len() - 1
            }
        };
        match by_key[i].1.iter_mut().find(|(p, _)| *p == pol) {
            Some(e) => e.1 = id,
            None => by_key[i].1.push((pol, id)),
        }
    }
    let mut new_pairs = group.pair_ids.clone();
    for (_, m) in by_key {
        let p = m.iter().find(|(k, _)| *k == "P").map(|(_, v)| *v);
        let n = m.iter().find(|(k, _)| *k == "N").map(|(_, v)| *v);
        match (p, n) {
            (Some(p), Some(n)) => new_pairs.push((p, n)),
            _ => new_ids.extend(m.iter().map(|(_, v)| *v)),
        }
    }
    new_ids.sort();
    new_pairs.sort();
    group.net_ids = new_ids;
    group.pair_ids = new_pairs;
}

/// `detect_match_groups(net_names, net_class_routing=..., net_to_class=...,
/// length_tracker=..., enable_suffix_inference=...)`. `legacy` is the
/// tracker's `{name: net_ids}` match groups, if any.
pub fn detect_match_groups(
    net_names: &[(i64, String)],
    routing: Option<&SynthRouting>,
    legacy: Option<&[(String, Vec<i64>)]>,
    enable_suffix_inference: bool,
) -> Vec<MatchGroup> {
    let mut claimed: Vec<i64> = Vec::new();
    let mut out: Vec<MatchGroup> = Vec::new();
    for mut g in gather_explicit_groups(net_names, routing) {
        g.net_ids.sort();
        g.reference_net_id = resolve_reference(&g, net_names, routing);
        claimed.extend(g.net_ids.iter().copied());
        out.push(g);
    }
    for (name, ids) in legacy.unwrap_or(&[]) {
        if ids.iter().any(|i| claimed.contains(i)) {
            continue;
        }
        let mut members = ids.clone();
        members.sort();
        members.dedup();
        if members.is_empty() {
            continue;
        }
        claimed.extend(members.iter().copied());
        out.push(MatchGroup {
            name: name.clone(),
            net_ids: members,
            reference_net_id: None,
            source: MatchGroupSource::LegacyApi,
            pair_ids: vec![],
        });
    }
    if enable_suffix_inference {
        let remaining: Vec<(i64, String)> = net_names
            .iter()
            .filter(|(i, _)| !claimed.contains(i))
            .cloned()
            .collect();
        for mut g in infer_suffix_groups(&remaining) {
            if g.net_ids.iter().any(|i| claimed.contains(i)) {
                continue;
            }
            g.net_ids.sort();
            claimed.extend(g.net_ids.iter().copied());
            out.push(g);
        }
    }
    for g in &mut out {
        extract_pair_ids(g, net_names);
    }
    out
}
