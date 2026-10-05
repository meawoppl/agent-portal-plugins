//! DRC-constraint sidecar emission for `kct check --emit-dru` /
//! `--emit-drc-constraints` (port of `check_cmd._emit_drc_sidecars`, issue
//! #4375): write `<board>.kicad_dru` (and with `emit_both` the merged
//! `<board>.kicad_pro`) from the exact rules the check enforced.

use std::path::{Path, PathBuf};

use super::dru_generator::merge_dru_floors;
use super::project_generator::{generate_project_dru, write_drc_constraints};
use super::DesignRules;
use crate::pyjson::{self, Json};
use crate::router::rules::NetClassMap;
use crate::router::rules::NetClassRouting;

/// Net classes deduplicated by name, first-seen order.
pub fn unique_net_classes(map: Option<&NetClassMap>) -> Vec<NetClassRouting> {
    let mut out: Vec<NetClassRouting> = Vec::new();
    for (_, nc) in map.into_iter().flatten() {
        if !out.iter().any(|c| c.name == nc.name) {
            out.push(nc.clone());
        }
    }
    out
}

/// Write the sidecars; `Err` carries the warning detail.
pub fn write_drc_sidecars(
    pcb_path: &Path,
    rules: &DesignRules,
    manufacturer_id: &str,
    layers: i64,
    copper_oz: f64,
    net_class_map: Option<&NetClassMap>,
    emit_both: bool,
) -> Result<Vec<PathBuf>, String> {
    let classes = unique_net_classes(net_class_map);
    if emit_both {
        return write_drc_constraints(
            pcb_path,
            rules,
            manufacturer_id,
            Some(layers),
            Some(copper_oz),
            true,
            &classes,
        )
        .map_err(|e| e.to_string());
    }
    let dru = pcb_path.with_extension("kicad_dru");
    let pro = pcb_path.with_extension("kicad_pro");
    let project = if pro.exists() {
        let text = std::fs::read_to_string(&pro).map_err(|e| e.to_string())?;
        pyjson::loads(&text).map_err(|e| {
            crate::utils::pyjsondecode::json_decode_error(&text).unwrap_or_else(|| e.to_string())
        })?
    } else {
        Json::obj()
    };
    let existing = if dru.exists() {
        Some(std::fs::read_to_string(&dru).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let content = merge_dru_floors(
        existing.as_deref(),
        &generate_project_dru(rules, &project, manufacturer_id, &classes),
        Some(&dru.display().to_string()),
    );
    std::fs::write(&dru, content).map_err(|e| e.to_string())?;
    Ok(vec![dru])
}
