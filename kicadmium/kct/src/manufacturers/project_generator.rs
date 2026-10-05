//! `.kicad_pro` DRC-constraint generation (port of
//! `kicad_tools.manufacturers.project_generator`): built-in minimum rules,
//! non-blocking severities, the `Default` netclass, and the combined
//! `.kicad_pro` + `.kicad_dru` writer.

use std::path::{Path, PathBuf};

use super::dru_generator::{generate_dru, merge_dru_floors, smd_pad_clearance_inert_reason};
use super::DesignRules;
use crate::jobj;
use crate::pyjson::{self, dumps_indent, Json};
use crate::router::rules::NetClassRouting;

const NON_BLOCKING_SEVERITIES: [(&str, &str); 2] = [
    ("lib_footprint_mismatch", "ignore"),
    ("isolated_copper", "warning"),
];
const MICRO_VIA_FLOOR_DIAMETER_MM: f64 = 0.2;
const MICRO_VIA_FLOOR_ANNULAR_MM: f64 = 0.05;
const MICRO_VIA_FLOOR_HOLE_MM: f64 = 0.1;

fn fl(v: f64) -> Json {
    Json::Float(v)
}

/// `build_default_netclass(rules)`.
pub fn build_default_netclass(r: &DesignRules) -> Json {
    jobj! {
        "bus_width" => Json::Int(12),
        "clearance" => fl(r.min_clearance_mm),
        "diff_pair_gap" => fl(0.25),
        "diff_pair_via_gap" => fl(0.25),
        "diff_pair_width" => fl(r.min_trace_width_mm),
        "line_style" => Json::Int(0),
        "microvia_diameter" => fl(MICRO_VIA_FLOOR_DIAMETER_MM),
        "microvia_drill" => fl(MICRO_VIA_FLOOR_HOLE_MM),
        "name" => "Default",
        "pcb_color" => "rgba(0, 0, 0, 0.000)",
        "schematic_color" => "rgba(0, 0, 0, 0.000)",
        "track_width" => fl(r.min_trace_width_mm),
        "via_diameter" => fl(r.min_via_diameter_mm),
        "via_drill" => fl(r.min_via_drill_mm),
        "wire_width" => Json::Int(6),
    }
}

/// `build_project_rules(rules)`.
pub fn build_project_rules(r: &DesignRules) -> Json {
    jobj! {
        "min_clearance" => fl(r.min_clearance_mm),
        "min_track_width" => fl(r.min_trace_width_mm),
        "min_via_diameter" => fl(r.min_via_diameter_mm),
        "min_microvia_diameter" => fl(MICRO_VIA_FLOOR_DIAMETER_MM),
        "min_via_annular_width" => fl(MICRO_VIA_FLOOR_ANNULAR_MM),
        "min_through_hole_diameter" => fl(r.min_hole_diameter_mm),
        "min_via_hole" => fl(r.min_via_drill_mm),
        "min_microvia_drill" => fl(MICRO_VIA_FLOOR_HOLE_MM),
        "min_hole_to_hole" => fl(r.min_hole_to_hole_mm),
        "min_copper_edge_clearance" => fl(r.min_copper_to_edge_mm),
        "min_silk_clearance" => fl(r.min_solder_mask_clearance_mm),
        "min_text_thickness" => fl(r.min_silkscreen_width_mm),
        "min_text_height" => fl(r.min_silkscreen_height_mm),
    }
}

fn severities() -> Json {
    let mut o = Json::obj();
    for (k, v) in NON_BLOCKING_SEVERITIES {
        o.set(k, v);
    }
    o
}

fn defaults(r: &DesignRules) -> Json {
    jobj! {
        "track_min_width" => fl(r.min_trace_width_mm),
        "clearance_min" => fl(r.min_clearance_mm),
        "via_min_diameter" => fl(r.min_via_diameter_mm),
        "via_min_drill" => fl(r.min_via_drill_mm),
    }
}

/// `build_project_data(rules, project_name, ...)`.
pub fn build_project_data(
    r: &DesignRules,
    project_name: &str,
    manufacturer_id: &str,
    layers: Option<i64>,
    copper_oz: Option<f64>,
) -> Json {
    let mut meta =
        jobj! { "filename" => format!("{project_name}.kicad_pro"), "version" => Json::Int(1) };
    if !manufacturer_id.is_empty() {
        meta.set("manufacturer", manufacturer_id);
    }
    if let Some(l) = layers {
        meta.set("layers", Json::Int(l));
    }
    if let Some(c) = copper_oz {
        meta.set("copper_oz", fl(c));
    }
    jobj! {
        "meta" => meta,
        "board" => jobj! {
            "design_settings" => jobj! {
                "rules" => build_project_rules(r),
                "rule_severities" => severities(),
                "defaults" => defaults(r),
            },
        },
        "net_settings" => jobj! {
            "classes" => Json::Arr(vec![build_default_netclass(r)]),
            "meta" => jobj! { "version" => Json::Int(3) },
        },
        "schematic" => jobj! { "meta" => jobj! { "version" => Json::Int(1) } },
        "sheets" => Json::Arr(vec![]),
        "text_variables" => Json::obj(),
    }
}

/// Python `str(x)` of a JSON value (for the preserve flag).
fn py_str(v: &Json) -> String {
    match v {
        Json::Str(s) => s.clone(),
        other => other.py_repr(),
    }
}

fn preserve_flag(project: &Json) -> bool {
    let flag = project
        .get("text_variables")
        .and_then(|t| t.get("KCT_PRESERVE_BOARD_RULES"))
        .map(py_str)
        .unwrap_or_else(|| "0".into())
        .to_lowercase();
    flag == "1" || flag == "true"
}

fn is_number(v: &Json) -> bool {
    matches!(v, Json::Int(_) | Json::Float(_) | Json::Bool(_))
}

/// Python `max(previous, value)`: the first argument wins ties.
fn py_max(prev: &Json, value: &Json) -> Json {
    let (a, b) = (
        prev.as_f64().unwrap_or(f64::NAN),
        value.as_f64().unwrap_or(f64::NAN),
    );
    if b > a {
        value.clone()
    } else {
        prev.clone()
    }
}

fn setdefault_obj<'a>(parent: &'a mut Json, key: &str) -> &'a mut Json {
    if parent.get(key).is_none() {
        parent.set(key, Json::obj());
    }
    parent.get_mut(key).unwrap()
}

fn apply_minima(target: &mut Json, values: &Json, preserve: bool) {
    let Json::Obj(items) = values else {
        return;
    };
    for (k, v) in items {
        let new = match target.get(k) {
            Some(prev) if preserve && is_number(prev) => py_max(prev, v),
            _ => v.clone(),
        };
        target.set(k, new);
    }
}

/// `merge_project_rules(project_data, rules)` (mutates in place).
pub fn merge_project_rules(project: &mut Json, r: &DesignRules) {
    let preserve = preserve_flag(project);
    {
        let board = setdefault_obj(project, "board");
        let settings = setdefault_obj(board, "design_settings");
        apply_minima(
            setdefault_obj(settings, "rules"),
            &build_project_rules(r),
            preserve,
        );
        let sev = setdefault_obj(settings, "rule_severities");
        for (k, v) in NON_BLOCKING_SEVERITIES {
            if !preserve || sev.get(k).is_none() {
                sev.set(k, v);
            }
        }
        apply_minima(setdefault_obj(settings, "defaults"), &defaults(r), preserve);
    }
    let net_settings = setdefault_obj(project, "net_settings");
    if net_settings.get("classes").is_none() {
        net_settings.set("classes", Json::Arr(vec![]));
    }
    let Some(Json::Arr(classes)) = net_settings.get_mut("classes") else {
        return;
    };
    match classes
        .iter_mut()
        .find(|c| c.get("name").and_then(Json::as_str) == Some("Default"))
    {
        None => classes.insert(0, build_default_netclass(r)),
        Some(c) => apply_minima(
            c,
            &jobj! {
                "clearance" => fl(r.min_clearance_mm),
                "track_width" => fl(r.min_trace_width_mm),
                "via_diameter" => fl(r.min_via_diameter_mm),
                "via_drill" => fl(r.min_via_drill_mm),
            },
            preserve,
        ),
    }
}

/// `json.dumps(s)` for a rule-name/condition string.
fn json_str(s: &str) -> String {
    pyjson::dumps(&Json::Str(s.to_string()))
}

/// `generate_project_dru(rules, project_data, ...)`.
pub fn generate_project_dru(
    r: &DesignRules,
    project: &Json,
    manufacturer_id: &str,
    net_classes: &[NetClassRouting],
) -> String {
    let mut rules = r.clone();
    let mut preserved = String::new();
    if preserve_flag(project) {
        let native = project
            .get("board")
            .and_then(|b| b.get("design_settings"))
            .and_then(|d| d.get("rules"));
        let nat = |k: &str| {
            native
                .and_then(|n| n.get(k))
                .and_then(Json::as_f64)
                .unwrap_or(0.0)
        };
        let m = |cur: f64, k: &str| if nat(k) > cur { nat(k) } else { cur };
        rules.min_trace_width_mm = m(r.min_trace_width_mm, "min_track_width");
        rules.min_clearance_mm = m(r.min_clearance_mm, "min_clearance");
        rules.min_via_drill_mm = m(r.min_via_drill_mm, "min_via_hole");
        rules.min_via_diameter_mm = m(r.min_via_diameter_mm, "min_via_diameter");
        rules.min_annular_ring_mm = m(r.min_annular_ring_mm, "min_via_annular_width");
        rules.min_copper_to_edge_mm = m(r.min_copper_to_edge_mm, "min_copper_edge_clearance");
        let mut classes: Vec<&Json> = project
            .get("net_settings")
            .and_then(|n| n.get("classes"))
            .and_then(Json::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        let clr = |c: &Json| c.get("clearance").and_then(Json::as_f64).unwrap_or(0.0);
        classes.sort_by(|a, b| clr(a).total_cmp(&clr(b)));
        for c in classes {
            let cl = if clr(c) > rules.min_clearance_mm {
                clr(c)
            } else {
                rules.min_clearance_mm
            };
            let raw = c.get("name").and_then(Json::as_str).unwrap_or("");
            let name = raw.replace('\\', "\\\\").replace('\'', "\\'");
            let cond = format!("A.NetClass == '{name}' || B.NetClass == '{name}'");
            preserved.push_str(&format!(
                "\n(rule {}\n  (condition {})\n  (constraint clearance (min {}mm)))\n",
                json_str(&format!("Reviewed clearance - {raw}")),
                json_str(&cond),
                pyjson::py_float_repr(cl)
            ));
        }
    }
    generate_dru(&rules, manufacturer_id, net_classes) + &preserved
}

fn installed_kicad_cli_version() -> Option<String> {
    use std::sync::OnceLock;
    static V: OnceLock<Option<String>> = OnceLock::new();
    V.get_or_init(|| {
        let cli = crate::cli::runner::find_kicad_cli()?;
        let out = std::process::Command::new(cli)
            .arg("version")
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    })
    .clone()
}

fn warn_if_smd_pad_clearance_is_inert(r: &DesignRules, dru_path: &Path) {
    let Some(floor) = r.min_smd_pad_clearance_mm else {
        return;
    };
    if let Some(reason) = smd_pad_clearance_inert_reason(installed_kicad_cli_version().as_deref()) {
        eprintln!(
            "{}: {reason} (floor: {} mm different-net SMD pad clearance)",
            dru_path.display(),
            crate::utils::pyfmt::format_g(floor, 4)
        );
    }
}

/// `write_drc_constraints(pcb_path, rules, ...)` (no `source_pcb_path`
/// staging): write/merge `<board>.kicad_pro` and, optionally, the
/// `<board>.kicad_dru` managed block.
pub fn write_drc_constraints(
    pcb_path: &Path,
    r: &DesignRules,
    manufacturer_id: &str,
    layers: Option<i64>,
    copper_oz: Option<f64>,
    write_dru: bool,
    net_classes: &[NetClassRouting],
) -> std::io::Result<Vec<PathBuf>> {
    let stem = pcb_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pro = pcb_path.with_extension("kicad_pro");
    let fresh = || build_project_data(r, &stem, manufacturer_id, layers, copper_oz);
    let project = if pro.exists() {
        match std::fs::read_to_string(&pro)
            .ok()
            .and_then(|t| pyjson::loads(&t).ok())
        {
            Some(mut p @ Json::Obj(_)) => {
                merge_project_rules(&mut p, r);
                if !manufacturer_id.is_empty() {
                    setdefault_obj(&mut p, "meta").set("manufacturer", manufacturer_id);
                }
                p
            }
            _ => fresh(),
        }
    } else {
        fresh()
    };
    std::fs::write(&pro, dumps_indent(&project, 2))?;
    let mut written = vec![pro];
    if write_dru {
        let dru = pcb_path.with_extension("kicad_dru");
        let existing = std::fs::read_to_string(&dru).ok();
        let content = merge_dru_floors(
            existing.as_deref(),
            &generate_project_dru(r, &project, manufacturer_id, net_classes),
            Some(&dru.display().to_string()),
        );
        std::fs::write(&dru, content)?;
        warn_if_smd_pad_clearance_is_inert(r, &dru);
        written.push(dru);
    }
    Ok(written)
}
