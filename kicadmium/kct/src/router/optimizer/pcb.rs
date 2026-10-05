//! Port of `kicad_tools.router.optimizer.pcb`: text-level segment/via
//! parsing, segment replacement, and the file-level `optimize_pcb` driver
//! (with the DRC-aware per-net rollback gate).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{anyhow, bail, Result};
use regex::Regex;

use super::config::{OptimizationConfig, OptimizationStats};
use super::geometry::{count_corners, total_length};
use crate::router::layers::Layer;
use crate::router::primitives::{Segment, Via};
use crate::sexp::Value;

/// Net name -> segments, in first-seen order (Python dict semantics).
pub type NetSegments = Vec<(String, Vec<Segment>)>;

static RE_NET_DECL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\(net\s+(\d+)\s+("(?:\\.|[^"\\])*")\)"#).unwrap());
static RE_TOKENS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[#;][^\n]*|"(?:\\.|[^"\\])*"|[()]|[^\s()]+"#).unwrap());
static RE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(start\s+([\d.eE+-]+)\s+([\d.eE+-]+)\)").unwrap());
static RE_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(end\s+([\d.eE+-]+)\s+([\d.eE+-]+)\)").unwrap());
static RE_WIDTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(width\s+([\d.eE+-]+)\)").unwrap());
static RE_LAYER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\(layer\s+"([^"]+)"\)"#).unwrap());
static RE_NET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(net\s+(\d+)\)").unwrap());
static RE_NET_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\(net\s+("(?:\\.|[^"\\])*")\)"#).unwrap());
static RE_VIA_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\(via\s+(micro|blind|buried)\b").unwrap());
static RE_AT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(at\s+([\d.eE+-]+)\s+([\d.eE+-]+)\)").unwrap());
static RE_SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(size\s+([\d.eE+-]+)\)").unwrap());
static RE_DRILL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(drill\s+([\d.eE+-]+)\)").unwrap());
static RE_LAYERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\(layers\s+"([^"]+)"\s+"([^"]+)"\)"#).unwrap());

/// Decode a quoted KiCad name token with the canonical s-expression parser.
fn decode_net_string(token: &str) -> String {
    match crate::sexp::parse(&format!("(name {token})")) {
        Ok(node) => match node.children.first().and_then(|c| c.value.as_ref()) {
            Some(Value::Str(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        },
        Err(_) => token.trim_matches('"').to_string(),
    }
}

/// Python `float()` for a regex-captured numeric token.
fn pyfloat(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

/// Net id -> name from every `(net N "name")` occurrence (later wins).
pub fn parse_net_names(pcb_text: &str) -> HashMap<i64, String> {
    parse_net_names_ordered(pcb_text).into_iter().collect()
}

/// [`parse_net_names`] in Python dict insertion order.
pub fn parse_net_names_ordered(pcb_text: &str) -> Vec<(i64, String)> {
    let mut names: Vec<(i64, String)> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for caps in RE_NET_DECL.captures_iter(pcb_text) {
        let Ok(id) = caps[1].parse::<i64>() else {
            continue;
        };
        let name = decode_net_string(&caps[2]);
        if name.is_empty() {
            continue;
        }
        match index.get(&id) {
            Some(&i) => names[i].1 = name,
            None => {
                index.insert(id, names.len());
                names.push((id, name));
            }
        }
    }
    names
}

fn net_tables(pcb_text: &str) -> (HashMap<i64, String>, HashMap<String, i64>) {
    let ordered = parse_net_names_ordered(pcb_text);
    let name_to_id = ordered.iter().map(|(id, n)| (n.clone(), *id)).collect();
    (ordered.into_iter().collect(), name_to_id)
}

/// Balanced `(keyword ...)` blocks as `(start, end, text)` byte spans.
pub fn extract_balanced_blocks(text: &str, keyword: &str) -> Result<Vec<(usize, usize, String)>> {
    let opener = format!("({keyword}");
    let bytes = text.as_bytes();
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None;
    let mut depth = 0i64;
    for m in RE_TOKENS.find_iter(text) {
        let token = m.as_str();
        match start {
            None => {
                let pos = m.start();
                let after = pos + opener.len();
                if token == "("
                    && text[pos..].starts_with(&opener)
                    && after < bytes.len()
                    && (text[after..]
                        .chars()
                        .next()
                        .is_some_and(char::is_whitespace)
                        || bytes[after] == b')')
                {
                    start = Some(pos);
                    depth = 1;
                }
            }
            Some(s) => {
                if token == "(" {
                    depth += 1;
                } else if token == ")" {
                    depth -= 1;
                    if depth == 0 {
                        blocks.push((s, m.end(), text[s..m.end()].to_string()));
                        start = None;
                    }
                }
            }
        }
    }
    if start.is_some() {
        bail!("Unbalanced {keyword} S-expression");
    }
    Ok(blocks)
}

/// Resolve the net referenced in a block (numeric or KiCad-10 name form).
pub fn resolve_block_net(
    block: &str,
    net_names: &HashMap<i64, String>,
    name_to_id: &HashMap<String, i64>,
) -> Option<(i64, String)> {
    if let Some(c) = RE_NET.captures(block) {
        let net: i64 = c[1].parse().ok()?;
        let name = net_names
            .get(&net)
            .cloned()
            .unwrap_or_else(|| format!("Net{net}"));
        return Some((net, name));
    }
    if let Some(c) = RE_NET_NAME.captures(block) {
        let name = decode_net_string(&c[1]);
        if name.is_empty() {
            return Some((0, "Net0".to_string()));
        }
        return Some((name_to_id.get(&name).copied().unwrap_or(0), name));
    }
    None
}

fn push_grouped<T>(groups: &mut Vec<(String, Vec<T>)>, key: &str, item: T) {
    if let Some((_, items)) = groups.iter_mut().find(|(k, _)| k == key) {
        items.push(item);
    } else {
        groups.push((key.to_string(), vec![item]));
    }
}

/// Segments grouped by net name (first-seen order).
pub fn parse_segments(pcb_text: &str) -> Result<NetSegments> {
    let (net_names, name_to_id) = net_tables(pcb_text);
    let mut out: NetSegments = Vec::new();
    for (_, _, block) in extract_balanced_blocks(pcb_text, "segment")? {
        let (Some(ms), Some(me), Some(mw), Some(ml), Some((net, net_name))) = (
            RE_START.captures(&block),
            RE_END.captures(&block),
            RE_WIDTH.captures(&block),
            RE_LAYER.captures(&block),
            resolve_block_net(&block, &net_names, &name_to_id),
        ) else {
            continue;
        };
        let parse =
            |s: &str| pyfloat(s).ok_or_else(|| anyhow!("could not convert string to float: '{s}'"));
        let layer = Layer::from_kicad_name(&ml[1]).unwrap_or(Layer::FCu);
        let seg = Segment {
            x1: parse(&ms[1])?,
            y1: parse(&ms[2])?,
            x2: parse(&me[1])?,
            y2: parse(&me[2])?,
            width: parse(&mw[1])?,
            layer,
            net,
            net_name: net_name.clone(),
        };
        push_grouped(&mut out, &net_name, seg);
    }
    Ok(out)
}

/// Vias grouped by net name (first-seen order).
pub fn parse_vias(pcb_text: &str) -> Result<Vec<(String, Vec<Via>)>> {
    let (net_names, name_to_id) = net_tables(pcb_text);
    let mut out = Vec::new();
    for (_, _, block) in extract_balanced_blocks(pcb_text, "via")? {
        let (Some(ma), Some(msz), Some(md), Some(ml), Some((net, net_name))) = (
            RE_AT.captures(&block),
            RE_SIZE.captures(&block),
            RE_DRILL.captures(&block),
            RE_LAYERS.captures(&block),
            resolve_block_net(&block, &net_names, &name_to_id),
        ) else {
            continue;
        };
        let parse =
            |s: &str| pyfloat(s).ok_or_else(|| anyhow!("could not convert string to float: '{s}'"));
        let is_micro = RE_VIA_TYPE
            .captures(&block)
            .is_some_and(|c| &c[1] == "micro");
        let via = Via {
            x: parse(&ma[1])?,
            y: parse(&ma[2])?,
            drill: parse(&md[1])?,
            diameter: parse(&msz[1])?,
            layers: (
                Layer::from_kicad_name(&ml[1]).unwrap_or(Layer::FCu),
                Layer::from_kicad_name(&ml[2]).unwrap_or(Layer::BCu),
            ),
            net,
            net_name: net_name.clone(),
            in_pad: false,
            is_micro,
        };
        push_grouped(&mut out, &net_name, via);
    }
    Ok(out)
}

/// Replace the original segments of every optimized net with the optimized
/// ones (appended before the final `)`).
pub fn replace_segments(
    pcb_text: &str,
    original: &NetSegments,
    optimized: &NetSegments,
) -> Result<String> {
    let mut remove_ids: HashSet<i64> = HashSet::new();
    for (name, segs) in original {
        if optimized.iter().any(|(n, _)| n == name) {
            if let Some(first) = segs.first() {
                remove_ids.insert(first.net);
            }
        }
    }
    let bytes = pcb_text.as_bytes();
    let mut spans = Vec::new();
    for (s, e, block) in extract_balanced_blocks(pcb_text, "segment")? {
        if let Some(c) = RE_NET.captures(&block) {
            if c[1].parse::<i64>().is_ok_and(|n| remove_ids.contains(&n)) {
                let mut trail = e;
                while trail < bytes.len() && matches!(bytes[trail], b' ' | b'\t' | b'\n' | b'\r') {
                    trail += 1;
                }
                spans.push((s, trail));
            }
        }
    }
    let mut result = if spans.is_empty() {
        pcb_text.to_string()
    } else {
        let mut parts = String::with_capacity(pcb_text.len());
        let mut prev = 0;
        for (s, e) in spans {
            // A trailing-whitespace span can overlap the next block start
            // only when blocks are adjacent; Python slices the same way.
            if s >= prev {
                parts.push_str(&pcb_text[prev..s]);
            }
            prev = prev.max(e);
        }
        parts.push_str(&pcb_text[prev..]);
        parts
    };

    let mut new_sexps = Vec::new();
    for (_, segs) in optimized {
        for seg in segs {
            new_sexps.push(seg.try_to_sexp(false)?);
        }
    }
    if !new_sexps.is_empty() {
        if let Some(pos) = result.rfind(')') {
            if pos > 0 {
                let indent = "  ";
                let content = format!("\n{indent}{}\n", new_sexps.join(&format!("\n{indent}")));
                result.insert_str(pos, &content);
            }
        }
    }
    Ok(result)
}

/// DRC rule categories whose growth always triggers rollback (#3138).
pub const PAD_VIOLATION_CATEGORIES: &[&str] = &["clearance_pad_segment", "clearance_pad_via"];

/// Error counts keyed by rule id, plus `"__total__"`.
pub type DrcCounts = HashMap<String, usize>;

/// Hook computing DRC error counts for a candidate board text.
pub type DrcCounter<'a> = &'a dyn Fn(&str) -> Result<DrcCounts>;

fn total(c: &DrcCounts) -> usize {
    c.get("__total__").copied().unwrap_or(0)
}

/// Any pad-violation category grew between `before` and `after`.
pub fn pad_violations_grew(before: &DrcCounts, after: &DrcCounts) -> bool {
    PAD_VIOLATION_CATEGORIES
        .iter()
        .any(|c| after.get(*c).copied().unwrap_or(0) > before.get(*c).copied().unwrap_or(0))
}

/// `_run_drc_error_count_by_category`: clearance + dimension DRC error
/// counts (by rule id, plus `"__total__"`) for a candidate board text.
pub fn drc_error_counts(
    pcb_text: &str,
    manufacturer: &str,
    layers: i64,
    copper_oz: f64,
) -> Result<DrcCounts> {
    use crate::validate::checker::{DRCChecker, DRCCheckerOptions};
    let pcb = crate::schema::pcb::Pcb::parse_str(pcb_text)?;
    let checker = DRCChecker::new(
        &pcb,
        DRCCheckerOptions {
            manufacturer: manufacturer.to_string(),
            layers,
            copper_oz,
            ..DRCCheckerOptions::default()
        },
    )?;
    let mut results = checker.check_clearances();
    results.merge(checker.check_dimensions());
    let mut counts: DrcCounts = HashMap::from([("__total__".to_string(), results.error_count())]);
    for v in results.errors() {
        *counts.entry(v.rule_id.clone()).or_default() += 1;
    }
    Ok(counts)
}

/// Optimize every net's segments in a board file.
///
/// `drc` supplies error counts when `config.drc_aware` is set (the
/// pure-Rust DRC checker); `None` with `drc_aware` is an error.
pub fn optimize_pcb(
    pcb_path: &Path,
    output_path: Option<&Path>,
    optimize_fn: &dyn Fn(&[Segment]) -> Vec<Segment>,
    config: &OptimizationConfig,
    net_filter: Option<&str>,
    dry_run: bool,
    drc: Option<DrcCounter<'_>>,
) -> Result<OptimizationStats> {
    let pcb_text = std::fs::read_to_string(pcb_path)?;
    let mut stats = OptimizationStats::default();
    let mut segments_by_net = parse_segments(&pcb_text)?;
    if let Some(filter) = net_filter.filter(|f| !f.is_empty()) {
        let needle = filter.to_lowercase();
        segments_by_net.retain(|(net, _)| net.to_lowercase().contains(&needle));
    }
    for (_, segs) in &segments_by_net {
        stats.segments_before += segs.len();
        stats.corners_before += count_corners(segs, config.tolerance);
        stats.length_before += total_length(segs);
    }

    let drc_enabled = config.drc_aware && config.drc_manufacturer.is_some();
    let counter = if drc_enabled {
        Some(drc.ok_or_else(|| anyhow!("DRC-aware mode requires a DRC checker"))?)
    } else {
        None
    };
    let mut baseline: DrcCounts = HashMap::from([("__total__".to_string(), 0)]);
    if let Some(count) = counter {
        baseline = count(&pcb_text)?;
        stats.drc_errors_before = total(&baseline);
    }

    let mut optimized: NetSegments = Vec::new();
    for (net, segs) in &segments_by_net {
        optimized.push((net.clone(), optimize_fn(segs)));
        stats.nets_optimized += 1;
    }

    if let Some(count) = counter {
        let full_text = replace_segments(&pcb_text, &segments_by_net, &optimized)?;
        let full_counts = count(&full_text)?;
        let mut full_errors = total(&full_counts);
        let baseline_errors = total(&baseline);
        if full_errors > baseline_errors || pad_violations_grew(&baseline, &full_counts) {
            let mut final_segments = optimized.clone();
            let mut current = full_counts.clone();
            for idx in 0..optimized.len() {
                let original = &segments_by_net[idx].1;
                if &optimized[idx].1 == original {
                    continue;
                }
                let mut trial = final_segments.clone();
                trial[idx].1 = original.clone();
                let trial_text = replace_segments(&pcb_text, &segments_by_net, &trial)?;
                let trial_counts = count(&trial_text)?;
                let trial_total = total(&trial_counts);
                let pad_shrunk = PAD_VIOLATION_CATEGORIES.iter().any(|c| {
                    trial_counts.get(*c).copied().unwrap_or(0)
                        < current.get(*c).copied().unwrap_or(0)
                });
                if trial_total < full_errors || pad_shrunk {
                    final_segments[idx].1 = original.clone();
                    stats.nets_rolled_back += 1;
                    full_errors = trial_total;
                    current = trial_counts;
                    if full_errors <= baseline_errors && !pad_violations_grew(&baseline, &current) {
                        break;
                    }
                }
            }
            optimized = final_segments;
        }
        stats.drc_errors_after = full_errors;
    }

    stats.segments_after = 0;
    stats.corners_after = 0;
    stats.length_after = 0.0;
    for (_, segs) in &optimized {
        stats.segments_after += segs.len();
        stats.corners_after += count_corners(segs, config.tolerance);
        stats.length_after += total_length(segs);
    }

    if !dry_run {
        let output = replace_segments(&pcb_text, &segments_by_net, &optimized)?;
        let out_path = output_path.unwrap_or(pcb_path);
        crate::fsutil::atomic_write(out_path, output.as_bytes())?;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r#"(kicad_pcb
  (net 0 "")
  (net 1 "GND")
  (net 2 "Net-(C1-Pad2)")
  (segment (start 0 0) (end 1 0) (width 0.25) (layer "F.Cu") (net 1) (uuid "a"))
  (segment (start 1 0) (end 2 0) (width 0.25) (layer "F.Cu") (net 1) (uuid "b"))
  (gr_text "(segment fake)" (at 0 0))
  (segment (uuid "c") (net 2) (layer "B.Cu") (width 0.2) (end 5 5) (start 4 4))
)
"#;

    #[test]
    fn parses_segments_in_any_field_order() {
        let segs = parse_segments(BOARD).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, "GND");
        assert_eq!(segs[0].1.len(), 2);
        assert_eq!(segs[1].0, "Net-(C1-Pad2)");
        assert_eq!(segs[1].1[0].layer, Layer::BCu);
        assert_eq!(segs[1].1[0].x1, 4.0);
    }

    #[test]
    fn quoted_parens_are_not_blocks() {
        let blocks = extract_balanced_blocks(BOARD, "segment").unwrap();
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn name_only_dialect_resolves() {
        let text = "(kicad_pcb (net 3 \"VCC\") (segment (start 0 0) (end 1 0) (width 0.2) (layer \"F.Cu\") (net \"VCC\")))";
        let segs = parse_segments(text).unwrap();
        assert_eq!(segs[0].1[0].net, 3);
    }
}
