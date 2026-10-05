//! Project-declared design-rule minima (kicadmium extension; upstream
//! kicad-tools always checks against the manufacturer profile).
//!
//! KiCad keeps a board's design constraints next to it: the
//! `<board>.kicad_pro` `board.design_settings.rules` minima, the
//! `net_settings.classes` clearances, and global rules in
//! `<board>.kicad_dru`. When these exist they are the board's authoritative
//! constraints: an auto-selected manufacturer profile (the `jlcpcb` default,
//! a fab-profile sidecar, or `project.kct target_fab`) must not override
//! them, otherwise `kct check` reports thousands of violations that KiCad's
//! own DRC (with the same project) does not.
//!
//! Only the generic constraints with a 1:1 KiCad counterpart replace the
//! profile values. Fabricator refinements without a KiCad board setting
//! (SMD-pad spacing, PTH hole-to-track, PTH annular ring, silk-to-pad) keep
//! the profile values, except that `min_pad_size_mm` is capped at the
//! project's minimum track width: the project already accepts copper
//! features that narrow.
//!
//! `.kicad_dru` rules are honoured only when unconditional (no `condition`
//! and no `layer` clause); conditional rules need KiCad's expression engine
//! and are left to `kicad-cli pcb drc`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::DesignRules;

/// One resolved project minimum, keyed by `DesignRules` field name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectRules {
    /// Files the minima came from (`.kicad_pro`, `.kicad_dru`).
    pub sources: Vec<PathBuf>,
    /// `DesignRules` field -> value in mm.
    pub minima: BTreeMap<&'static str, f64>,
    /// Non-fatal problems reading the project files.
    pub warnings: Vec<String>,
}

impl ProjectRules {
    pub fn is_empty(&self) -> bool {
        self.minima.is_empty()
    }

    /// Replace profile values with the project minima.
    pub fn apply(&self, rules: &DesignRules) -> DesignRules {
        let mut out = rules.clone();
        for (&field, &v) in &self.minima {
            match field {
                "min_clearance_mm" => out.min_clearance_mm = v,
                "min_trace_width_mm" => out.min_trace_width_mm = v,
                "min_via_diameter_mm" => out.min_via_diameter_mm = v,
                "min_via_drill_mm" => out.min_via_drill_mm = v,
                "min_annular_ring_mm" => out.min_annular_ring_mm = v,
                "min_hole_diameter_mm" => out.min_hole_diameter_mm = v,
                "min_hole_to_hole_mm" => out.min_hole_to_hole_mm = v,
                "min_copper_to_edge_mm" => out.min_copper_to_edge_mm = v,
                "min_silkscreen_height_mm" => out.min_silkscreen_height_mm = v,
                _ => {}
            }
        }
        if let Some(&w) = self.minima.get("min_trace_width_mm") {
            if out.min_pad_size_mm > w {
                out.min_pad_size_mm = w;
            }
        }
        out
    }

    /// `field=value, ...` for the stderr rule-source line.
    pub fn describe(&self) -> String {
        self.minima
            .iter()
            .map(|(k, v)| format!("{k}={}", crate::pyjson::py_float_repr(*v)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn source_names(&self) -> String {
        self.sources
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string())
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

fn positive(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|x| x.is_finite() && *x > 0.0)
}

fn raise(minima: &mut BTreeMap<&'static str, f64>, field: &'static str, v: f64) {
    let e = minima.entry(field).or_insert(v);
    if v > *e {
        *e = v;
    }
}

/// Parse `.kicad_pro` JSON text: board minima plus net-class clearances.
pub fn parse_kicad_pro(text: &str) -> Result<BTreeMap<&'static str, f64>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut minima = BTreeMap::new();
    let rules = root
        .pointer("/board/design_settings/rules")
        .cloned()
        .unwrap_or(Value::Null);
    let r = |k: &str| positive(rules.get(k));
    for (key, field) in [
        ("min_track_width", "min_trace_width_mm"),
        ("min_via_diameter", "min_via_diameter_mm"),
        ("min_via_annular_width", "min_annular_ring_mm"),
        ("min_hole_to_hole", "min_hole_to_hole_mm"),
        ("min_copper_edge_clearance", "min_copper_to_edge_mm"),
        ("min_text_height", "min_silkscreen_height_mm"),
    ] {
        if let Some(v) = r(key) {
            minima.insert(field, v);
        }
    }
    // KiCad 7+ `min_through_hole_diameter` covers via drills and PTH holes;
    // KiCad 6 wrote `min_via_hole` for vias only.
    if let Some(v) = r("min_through_hole_diameter") {
        minima.insert("min_hole_diameter_mm", v);
        minima.insert("min_via_drill_mm", v);
    } else if let Some(v) = r("min_via_hole") {
        minima.insert("min_via_drill_mm", v);
    }
    // KiCad enforces max(board minimum, net-class clearance) per item pair;
    // the loosest pair on the board sits at the smallest class clearance.
    let class_min = root
        .pointer("/net_settings/classes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| positive(c.get("clearance")))
        .reduce(f64::min);
    match (r("min_clearance"), class_min) {
        (Some(b), Some(c)) => {
            minima.insert("min_clearance_mm", b.max(c));
        }
        (Some(v), None) | (None, Some(v)) => {
            minima.insert("min_clearance_mm", v);
        }
        (None, None) => {}
    }
    Ok(minima)
}

/// KiCad DRU length (`0.2mm`, `6mil`, `0.01in`, `150um`, bare = mm).
fn parse_length(raw: &str) -> Option<f64> {
    let s = raw.trim();
    for (suffix, scale) in [
        ("mm", 1.0),
        ("mil", 0.0254),
        ("um", 0.001),
        ("in", 25.4),
        ("\"", 25.4),
    ] {
        if let Some(n) = s.strip_suffix(suffix) {
            return n.trim().parse::<f64>().ok().map(|v| v * scale);
        }
    }
    s.parse::<f64>().ok()
}

/// Unconditional `.kicad_dru` minima (rules without `condition`/`layer`).
pub fn parse_kicad_dru(text: &str) -> Result<BTreeMap<&'static str, f64>, String> {
    let exprs = crate::sexp::parse_all(text).map_err(|e| e.to_string())?;
    let mut minima = BTreeMap::new();
    for rule in exprs.iter().filter(|e| e.has_tag("rule")) {
        if rule.find_child("condition").is_some() || rule.find_child("layer").is_some() {
            continue;
        }
        for c in rule.find_children("constraint") {
            let Some(kind) = c.text_at(0) else { continue };
            let Some(min) = c
                .find_child("min")
                .and_then(|m| m.text_at(0))
                .and_then(|t| parse_length(&t))
                .filter(|v| *v > 0.0)
            else {
                continue;
            };
            let fields: &[&'static str] = match kind.as_str() {
                "clearance" => &["min_clearance_mm"],
                "track_width" => &["min_trace_width_mm"],
                "via_diameter" => &["min_via_diameter_mm"],
                "annular_width" => &["min_annular_ring_mm"],
                "hole_size" => &["min_hole_diameter_mm", "min_via_drill_mm"],
                "hole_to_hole" => &["min_hole_to_hole_mm"],
                "edge_clearance" => &["min_copper_to_edge_mm"],
                "text_height" => &["min_silkscreen_height_mm"],
                _ => &[],
            };
            for f in fields {
                raise(&mut minima, f, min);
            }
        }
    }
    Ok(minima)
}

/// Discover and merge `<board>.kicad_pro` and `<board>.kicad_dru`.
///
/// Board minima are always enforced by KiCad, so an unconditional DRU
/// constraint can only tighten them (`max`); an unconditional DRU
/// `clearance` replaces the net-class clearance, but not the board minimum.
pub fn load_project_rules(pcb_path: &Path) -> ProjectRules {
    let mut out = ProjectRules::default();
    let pro = pcb_path.with_extension("kicad_pro");
    let mut board_min_clearance = None;
    if pro.is_file() {
        let parsed = std::fs::read_to_string(&pro)
            .map_err(|e| e.to_string())
            .and_then(|t| {
                let board_min = serde_json::from_str::<Value>(&t).ok().and_then(|v| {
                    positive(v.pointer("/board/design_settings/rules/min_clearance"))
                });
                parse_kicad_pro(&t).map(|m| (m, board_min))
            });
        match parsed {
            Ok((m, board_min)) => {
                if !m.is_empty() {
                    out.sources.push(pro.clone());
                }
                board_min_clearance = board_min;
                out.minima = m;
            }
            Err(e) => out.warnings.push(format!(
                "ignoring unreadable project file {}: {e}",
                pro.display()
            )),
        }
    }
    let dru = pcb_path.with_extension("kicad_dru");
    if dru.is_file() {
        match std::fs::read_to_string(&dru)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_kicad_dru(&t))
        {
            Ok(m) => {
                if !m.is_empty() {
                    out.sources.push(dru.clone());
                }
                for (field, v) in m {
                    if field == "min_clearance_mm" {
                        let floor = board_min_clearance.unwrap_or(0.0);
                        out.minima.insert(field, v.max(floor));
                    } else {
                        raise(&mut out.minima, field, v);
                    }
                }
            }
            Err(e) => out.warnings.push(format!(
                "ignoring unparseable design-rule file {}: {e}",
                dru.display()
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicad_pro_minima_map_to_design_rule_fields() {
        let m = parse_kicad_pro(
            r#"{"board":{"design_settings":{"rules":{
                "min_clearance":0.1,"min_track_width":0.1,"min_hole_to_hole":0.25,
                "min_via_diameter":0.45,"min_via_annular_width":0.1,
                "min_through_hole_diameter":0.2,"min_copper_edge_clearance":0.0}}},
              "net_settings":{"classes":[{"name":"Default","clearance":0.1}]}}"#,
        )
        .unwrap();
        assert_eq!(m["min_clearance_mm"], 0.1);
        assert_eq!(m["min_trace_width_mm"], 0.1);
        assert_eq!(m["min_hole_to_hole_mm"], 0.25);
        assert_eq!(m["min_via_diameter_mm"], 0.45);
        assert_eq!(m["min_annular_ring_mm"], 0.1);
        assert_eq!(m["min_via_drill_mm"], 0.2);
        assert_eq!(m["min_hole_diameter_mm"], 0.2);
        // 0 means "no constraint" in KiCad: keep the profile value.
        assert!(!m.contains_key("min_copper_to_edge_mm"));
    }

    #[test]
    fn netclass_clearance_raises_zero_board_minimum() {
        let m = parse_kicad_pro(
            r#"{"board":{"design_settings":{"rules":{"min_clearance":0.0}}},
              "net_settings":{"classes":[{"clearance":0.2},{"clearance":0.3}]}}"#,
        )
        .unwrap();
        assert_eq!(m["min_clearance_mm"], 0.2);
    }

    #[test]
    fn dru_only_unconditional_rules_count() {
        let m = parse_kicad_dru(
            "(version 1)\n# comment\n\
             (rule a (constraint clearance (min 0.15mm)))\n\
             (rule b (condition \"A.Type == 'Via'\") (constraint hole_clearance (min 0.3mm)))\n\
             (rule c (layer inner) (constraint track_width (min 0.3mm)))\n\
             (rule d (constraint hole_to_hole (min 10mil)))\n",
        )
        .unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m["min_clearance_mm"], 0.15);
        assert!((m["min_hole_to_hole_mm"] - 0.254).abs() < 1e-12);
    }

    #[test]
    fn apply_caps_pad_size_at_project_track_width() {
        let base = crate::manufacturers::rules("jlcpcb", 4, 1.0).unwrap();
        let mut pr = ProjectRules::default();
        pr.minima.insert("min_trace_width_mm", 0.1);
        pr.minima.insert("min_hole_to_hole_mm", 0.25);
        let r = pr.apply(&base);
        assert_eq!(r.min_trace_width_mm, 0.1);
        assert_eq!(r.min_pad_size_mm, 0.1);
        assert_eq!(r.min_hole_to_hole_mm, 0.25);
        assert_eq!(r.min_clearance_mm, base.min_clearance_mm);
    }
}
