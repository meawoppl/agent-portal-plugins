//! Schematic field-geometry lint (port of
//! `kicad_tools.validate.rules.schematic_fields`, issue #4595): visible
//! Reference/Value fields far from their symbol body (`sch_field_offset`)
//! or overlapping another symbol's body / field text (`sch_field_overlap`).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::pyjson::py_round;
use crate::schema::field_geometry::{field_offset_mm, placed_body_bbox, BBox};
use crate::schema::schematic::Schematic;
use crate::schema::symbol::SymbolInstance;
use crate::sexp::{SExp, Value};
use crate::validate::violations::{DRCResults, DRCViolation};

pub const DEFAULT_SCH_FIELD_THRESHOLD_MM: f64 = 15.0;
pub const RULE_FIELD_OFFSET: &str = "sch_field_offset";
pub const RULE_FIELD_OVERLAP: &str = "sch_field_overlap";
pub const LINT_FIELDS: [&str; 2] = ["Reference", "Value"];
pub const DEFAULT_FONT_HEIGHT_MM: f64 = 1.27;
pub const CHAR_WIDTH_FACTOR: f64 = 0.8;

/// Heuristic axis-aligned bbox of a rendered field text.
pub fn field_text_bbox(
    text: &str,
    at: (f64, f64),
    rotation: f64,
    font_height_mm: f64,
    justify: &str,
) -> BBox {
    let (x, y) = at;
    let length = text.chars().count().max(1) as f64 * CHAR_WIDTH_FACTOR * font_height_mm;
    let h = font_height_mm;
    if rotation.rem_euclid(180.0) == 90.0 {
        return (x - h / 2.0, y - length / 2.0, x + h / 2.0, y + length / 2.0);
    }
    let (min_x, max_x) = match justify {
        "left" => (x, x + length),
        "right" => (x - length, x),
        _ => (x - length / 2.0, x + length / 2.0),
    };
    (min_x, y - h / 2.0, max_x, y + h / 2.0)
}

fn overlap(a: BBox, b: BBox) -> bool {
    a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1
}

fn is_hidden(prop: &SExp) -> bool {
    let Some(effects) = prop.get("effects") else {
        return false;
    };
    effects.get("hide").is_some()
        || effects
            .children
            .iter()
            .any(|c| c.is_atom() && matches!(&c.value, Some(Value::Str(s)) if s == "hide"))
}

fn font_height_mm(prop: &SExp) -> f64 {
    prop.get("effects")
        .and_then(|e| e.get("font"))
        .and_then(|f| f.get("size"))
        .and_then(|s| s.float_at(0))
        .filter(|h| *h > 0.0)
        .unwrap_or(DEFAULT_FONT_HEIGHT_MM)
}

fn justify(prop: &SExp) -> String {
    prop.get("effects")
        .and_then(|e| e.get("justify"))
        .and_then(|j| {
            j.children.iter().find_map(|c| match &c.value {
                Some(Value::Str(s)) if c.is_atom() && (s == "left" || s == "right") => {
                    Some(s.clone())
                }
                _ => None,
            })
        })
        .unwrap_or_default()
}

struct FieldRecord {
    reference: String,
    field_name: String,
    position: (f64, f64),
    text_bbox: BBox,
}

fn resolve(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Root sheet plus every reachable sub-sheet file (breadth-first, once each).
fn sheet_files(root: &Path) -> Vec<PathBuf> {
    let mut ordered = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::from([resolve(root)]);
    while let Some(sheet) = queue.pop_front() {
        if !visited.insert(sheet.clone()) {
            continue;
        }
        if !sheet.exists() {
            eprintln!(
                "Warning: sch_fields: sheet file not found: {}; skipped",
                sheet.display()
            );
            continue;
        }
        ordered.push(sheet.clone());
        let sch = match Schematic::load(&sheet) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Warning: sch_fields: could not parse {} for sheet enumeration ({e})",
                    file_name(&sheet)
                );
                continue;
            }
        };
        for s in sch.sheets() {
            if s.filename.is_empty() {
                continue;
            }
            let mut child = PathBuf::from(&s.filename);
            if !child.is_absolute() {
                child = sheet.parent().unwrap_or(Path::new("")).join(child);
            }
            queue.push_back(resolve(&child));
        }
    }
    ordered
}

type Key = (String, String, String, String, String, String);

fn lint_sheet(sheet: &Path, threshold_mm: f64) -> Vec<(Key, DRCViolation)> {
    let sch = match Schematic::load(sheet) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Warning: sch_fields: could not parse {} ({e}); skipped",
                file_name(sheet)
            );
            return vec![];
        }
    };
    let sheet_name = file_name(sheet);
    let sheet_str = sheet.display().to_string();
    let key = |r: &str, f: &str, rule: &str, m: &str| -> Key {
        (
            sheet_name.clone(),
            sheet_str.clone(),
            r.into(),
            f.into(),
            rule.into(),
            m.into(),
        )
    };
    let mut findings = Vec::new();
    let mut bodies: Vec<(String, BBox)> = Vec::new();
    let mut fields: Vec<FieldRecord> = Vec::new();
    for sym in sch.sexp().children.iter().filter(|c| c.has_tag("symbol")) {
        let inst = SymbolInstance::from_sexp(sym);
        let reference = inst.reference().to_string();
        if reference.starts_with('#') {
            continue;
        }
        let lib = match sch.get_lib_symbol_resolved(&inst.lib_id) {
            Ok(Some(l)) => l,
            _ => {
                eprintln!(
                    "Warning: sch_fields: {sheet_name}: {reference}: lib_id '{}' not found in \
                     embedded lib_symbols; skipped",
                    inst.lib_id
                );
                continue;
            }
        };
        let bbox = placed_body_bbox(
            &lib,
            inst.position,
            inst.rotation,
            &inst.mirror,
            Some(inst.unit),
        );
        for prop in sym.children.iter().filter(|c| c.has_tag("property")) {
            let name = prop.text_at(0).unwrap_or_default();
            if !LINT_FIELDS.contains(&name.as_str()) || is_hidden(prop) {
                continue;
            }
            let Some(at) = prop.get("at") else {
                continue;
            };
            let pos = (at.float_at(0).unwrap_or(0.0), at.float_at(1).unwrap_or(0.0));
            let rot = at.float_at(2).unwrap_or(0.0);
            let text = prop.text_at(1).unwrap_or_default();
            let offset = field_offset_mm(pos, bbox);
            if offset > threshold_mm {
                let message = format!(
                    "{reference}.{name} {offset:.1}mm from body (threshold {threshold_mm:.1}mm) \
                     [{sheet_name}]"
                );
                findings.push((
                    key(&reference, &name, RULE_FIELD_OFFSET, &message),
                    DRCViolation::new(RULE_FIELD_OFFSET, "warning", message)
                        .at(pos.0, pos.1)
                        .actual(py_round(offset, 3))
                        .required(threshold_mm)
                        .items([reference.clone()]),
                ));
            }
            fields.push(FieldRecord {
                reference: reference.clone(),
                field_name: name.clone(),
                position: pos,
                text_bbox: field_text_bbox(&text, pos, rot, font_height_mm(prop), &justify(prop)),
            });
        }
        bodies.push((reference, bbox));
    }
    bodies.sort_by(|a, b| a.0.cmp(&b.0));
    fields.sort_by(|a, b| (&a.reference, &a.field_name).cmp(&(&b.reference, &b.field_name)));
    for r in &fields {
        for (body_ref, body) in &bodies {
            if *body_ref == r.reference || !overlap(r.text_bbox, *body) {
                continue;
            }
            let message = format!(
                "{}.{} text overlaps {body_ref} body [{sheet_name}]",
                r.reference, r.field_name
            );
            findings.push((
                key(&r.reference, &r.field_name, RULE_FIELD_OVERLAP, &message),
                DRCViolation::new(RULE_FIELD_OVERLAP, "warning", message)
                    .at(r.position.0, r.position.1)
                    .items([r.reference.clone(), body_ref.clone()]),
            ));
        }
    }
    for (i, a) in fields.iter().enumerate() {
        for b in &fields[i + 1..] {
            if a.reference == b.reference || !overlap(a.text_bbox, b.text_bbox) {
                continue;
            }
            let message = format!(
                "{}.{} text overlaps {}.{} text [{sheet_name}]",
                a.reference, a.field_name, b.reference, b.field_name
            );
            findings.push((
                key(&a.reference, &a.field_name, RULE_FIELD_OVERLAP, &message),
                DRCViolation::new(RULE_FIELD_OVERLAP, "warning", message)
                    .at(a.position.0, a.position.1)
                    .items([a.reference.clone(), b.reference.clone()]),
            ));
        }
    }
    findings
}

/// Run the schematic field-geometry lint over the root sheet and sub-sheets.
pub fn check_schematic_fields(sch_path: &Path, threshold_mm: f64) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(2);
    results.set_rule(RULE_FIELD_OFFSET, 1);
    results.set_rule(RULE_FIELD_OVERLAP, 1);
    let mut findings: Vec<(Key, DRCViolation)> = Vec::new();
    for sheet in sheet_files(sch_path) {
        findings.extend(lint_sheet(&sheet, threshold_mm));
    }
    findings.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, v) in findings {
        results.add(v);
    }
    results
}
