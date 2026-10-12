//! Explicitly partial local checks with content-addressed region caches.
use crate::{
    region::{query, Bounds},
    schema::pcb::Pcb,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};
const LOCAL_RULES: &[&str] = &[
    "trace.short_segment",
    "trace.minimum_width",
    "trace.mergeable",
    "trace.width_transition",
    "via.drill",
    "via.pad_overlap",
    "copper.duplicate_overlap",
    "pad.grazing",
];
fn scoped(source: &str, bounds: Bounds) -> Result<String> {
    let inventory = query(source, bounds, &[], &[], 0)?;
    let ids: HashSet<_> = inventory
        .items
        .iter()
        .filter_map(|v| v["id"].as_str())
        .collect();
    let mut tree = crate::sexp::parse(source)?;
    tree.children.retain(|n| {
        !matches!(
            n.tag(),
            Some("footprint" | "segment" | "arc" | "via" | "zone")
        ) || n.child_str("uuid").is_none_or(|id| ids.contains(id))
    });
    Ok(tree.to_kicad_string())
}
fn located(v: &Value, b: Bounds) -> bool {
    let p = v.get("at").or_else(|| v.get("location"));
    let Some(p) = p else { return true };
    let x = p["x"].as_f64().or_else(|| p[0].as_f64());
    let y = p["y"].as_f64().or_else(|| p[1].as_f64());
    match (x, y) {
        (Some(x), Some(y)) => b.contains(crate::lint::model::Point { x, y }),
        _ => true,
    }
}
pub fn run(
    board: &Path,
    bounds: Bounds,
    margin: f64,
    kind: &str,
    mut config: crate::lint::Config,
) -> Result<Value> {
    bounds.validate()?;
    if !margin.is_finite() || margin < 0. {
        bail!("region margin must be nonnegative and finite")
    }
    let expanded = Bounds {
        x0: bounds.x0 - margin,
        y0: bounds.y0 - margin,
        x1: bounds.x1 + margin,
        y1: bounds.y1 + margin,
    };
    let source = std::fs::read_to_string(board)?;
    let local = scoped(&source, expanded)?;
    let mut material = format!(
        "regional/v1:{}:{kind}:{}:{bounds:?}:{margin}:{local}",
        env!("CARGO_PKG_VERSION"),
        serde_json::to_string(&config)?
    );
    for extension in ["kicad_pro", "kicad_dru", "lint.json"] {
        let p = board.with_extension(extension);
        if p.exists() {
            material.push_str(&std::fs::read_to_string(p)?);
        }
    }
    let key = crate::lint::hash(&material);
    let cache_dir = board
        .parent()
        .unwrap_or(Path::new("."))
        .join(".kct-region-cache");
    let cache = cache_dir.join(format!("{key}.json"));
    let mut report: Value = if cache.exists() {
        let mut r: Value = serde_json::from_slice(&std::fs::read(&cache)?)?;
        r["cache_hit"] = json!(true);
        r
    } else {
        let findings = match kind {
            "lint" => {
                let allowed: std::collections::BTreeSet<_> =
                    LOCAL_RULES.iter().map(|s| s.to_string()).collect();
                config.enabled_only = if config.enabled_only.is_empty() {
                    allowed
                } else {
                    config
                        .enabled_only
                        .intersection(&allowed)
                        .cloned()
                        .collect()
                };
                if config.enabled_only.is_empty() {
                    json!([])
                } else {
                    serde_json::to_value(crate::lint::lint(&local, "regional", config)?.findings)?
                }
            }
            "drc" => {
                let p = Pcb::new(crate::sexp::parse(&local)?, Some(board.to_path_buf()))?;
                let checker = crate::validate::DRCChecker::new(&p, Default::default())?;
                if margin < checker.design_rules.min_clearance_mm {
                    bail!("region margin is smaller than project clearance")
                }
                let dru = board.with_extension("kicad_dru");
                if dru.exists() {
                    for r in crate::sexp::parse_all(&std::fs::read_to_string(dru)?)? {
                        if r.has_tag("rule")
                            && (r.get("condition").is_some() || r.get("layer").is_some())
                        {
                            bail!("regional native DRC cannot evaluate conditional custom rules; run full DRC")
                        }
                    }
                }
                let mut f = checker.check_clearances().violations;
                f.extend(checker.check_segment_zone_clearances().violations);
                f.extend(checker.check_via_zone_clearances().violations);
                f.extend(checker.check_courtyard_overlap().violations);
                json!(f.iter().map(|f|json!({"rule":f.rule_id,"severity":f.severity,"message":f.message,"at":f.location,"nets":f.nets,"refs":f.items})).collect::<Vec<_>>())
            }
            _ => bail!("unknown regional check"),
        };
        let findings: Vec<_> = findings
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| located(f, bounds))
            .cloned()
            .collect();
        let result = json!({"schema":"kicadmium.regional-check/v1","tool":kind,"engine":"native-local","full_board_gate":false,"requires_full_verification":true,"region":bounds,"margin_mm":margin,"cache_hit":false,"coverage":if kind=="lint"{json!(LOCAL_RULES)}else{json!(["copper-clearance","zone-clearance","courtyard-overlap"])},"findings":findings});
        std::fs::create_dir_all(&cache_dir)?;
        crate::fsutil::atomic_write(&cache, &serde_json::to_vec(&result)?)?;
        result
    };
    report["board_sha256"] = json!(crate::lint::hash(&source));
    report["ok"] = json!(!report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["severity"] == "error"));
    Ok(report)
}
