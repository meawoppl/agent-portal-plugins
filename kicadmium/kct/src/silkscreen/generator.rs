//! Port of `kicad_tools.silkscreen.generator`: ensure reference designators
//! are visible on silkscreen and add board-level markings (project name,
//! revision, date).
//!
//! Marking identity is persisted in a sibling `<pcb>.kct.json` sidecar keyed
//! by the `gr_text` UUID, so renames and revision bumps replace the old
//! marking instead of accumulating stale text. The PCB itself stays
//! KiCad-conformant.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::core::sexp_file::save_pcb;
use crate::drc::repair_silkscreen::{footprint_paths, node_at, node_at_mut, NodePath};
use crate::pyjson::{py_round, Json};
use crate::sexp::{SExp, Value};

/// Silkscreen layer names recognised by KiCad (pre-8.0 and 8.0+).
pub const SILKSCREEN_LAYERS: &[&str] = &["F.SilkS", "B.SilkS", "F.Silkscreen", "B.Silkscreen"];

const SIDECAR_VERSION: i64 = 1;

/// Result of a silkscreen generation pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SilkscreenResult {
    pub refs_unhidden: usize,
    pub markings_added: usize,
    pub markings_skipped: usize,
    pub messages: Vec<String>,
}

impl SilkscreenResult {
    pub fn total_changes(&self) -> usize {
        self.refs_unhidden + self.markings_added
    }
}

/// One sidecar registry row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkingEntry {
    pub uuid: String,
    pub tag: String,
    pub added_at: Option<String>,
}

/// `kicad_tools.sexp.builders.fmt`: round to 2 decimals, int when whole.
pub fn fmt_coord(v: f64) -> Value {
    let r = py_round(v, 2);
    if r == r.trunc() && r.abs() < 1e15 {
        Value::Int(r as i64)
    } else {
        Value::Float(r)
    }
}

/// `kicad_tools.sexp.builders.gr_text_node`.
pub fn gr_text_node(
    text: &str,
    x: f64,
    y: f64,
    layer: &str,
    font_size: f64,
    font_thickness: f64,
    uuid_str: &str,
) -> SExp {
    let effects = SExp::list(
        "effects",
        [SExp::list(
            "font",
            [
                SExp::list(
                    "size",
                    [
                        SExp::atom(fmt_coord(font_size)),
                        SExp::atom(fmt_coord(font_size)),
                    ],
                ),
                SExp::list("thickness", [SExp::atom(fmt_coord(font_thickness))]),
            ],
        )],
    );
    SExp::list(
        "gr_text",
        [
            SExp::atom(text),
            SExp::list("at", [SExp::atom(fmt_coord(x)), SExp::atom(fmt_coord(y))]),
            SExp::list("layer", [SExp::atom(layer)]),
            SExp::list("uuid", [SExp::atom(uuid_str)]),
            effects,
        ],
    )
}

/// Silkscreen content generator over a parsed board.
#[derive(Debug, Clone)]
pub struct SilkscreenGenerator {
    pub path: PathBuf,
    pub doc: SExp,
    registry: Vec<MarkingEntry>,
}

fn atom_text(v: &Value) -> String {
    v.to_string()
}

fn first_atom_is(node: &SExp, s: &str) -> bool {
    node.first_atom().is_some_and(|a| atom_text(a) == s)
}

fn is_hide_atom(c: &SExp) -> bool {
    c.is_atom() && c.value.as_ref().is_some_and(|v| atom_text(v) == "hide")
}

/// Python `datetime.now(timezone.utc).isoformat()`.
fn utc_now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let micros = now.subsec_micros();
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let frac = if micros == 0 {
        String::new()
    } else {
        format!(".{micros:06}")
    };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{frac}+00:00",
        tod / 3600,
        tod / 60 % 60,
        tod % 60
    )
}

impl SilkscreenGenerator {
    pub fn new(pcb_path: impl AsRef<Path>) -> Result<Self> {
        let path = pcb_path.as_ref().to_path_buf();
        let doc = crate::sexp::parse_file(&path)?;
        let registry = Self::load_sidecar(&Self::sidecar_for(&path));
        Ok(SilkscreenGenerator {
            path,
            doc,
            registry,
        })
    }

    fn sidecar_for(path: &Path) -> PathBuf {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        path.with_file_name(format!("{name}.kct.json"))
    }

    /// The sibling `<pcb>.kct.json` path used to track markings.
    pub fn sidecar_path(&self) -> PathBuf {
        Self::sidecar_for(&self.path)
    }

    /// Current marking registry.
    pub fn registry(&self) -> &[MarkingEntry] {
        &self.registry
    }

    /// Unhide footprint reference designators on silkscreen layers.
    pub fn ensure_ref_des_visible(&mut self) -> SilkscreenResult {
        let mut result = SilkscreenResult::default();
        for fp_path in footprint_paths(&self.doc) {
            let fp = node_at(&self.doc, &fp_path);
            let Some(ri) = Self::find_ref_text(fp) else {
                continue;
            };
            let text = &fp.children[ri];
            if !Self::is_silkscreen(text) || !Self::is_hidden(text) {
                continue;
            }
            let name = Self::extract_ref_name(fp);
            let mut p: NodePath = fp_path.clone();
            p.push(ri);
            Self::unhide(node_at_mut(&mut self.doc, &p));
            result.refs_unhidden += 1;
            result.messages.push(format!("Unhid reference {name}"));
        }
        result
    }

    /// Add board-level text markings (project name + revision, date).
    #[allow(clippy::too_many_arguments)]
    pub fn add_board_markings(
        &mut self,
        name: Option<&str>,
        revision: Option<&str>,
        date: Option<&str>,
        layer: &str,
        font_size: f64,
        font_thickness: f64,
    ) -> SilkscreenResult {
        let mut result = SilkscreenResult::default();
        let mut markings: Vec<(&str, String)> = Vec::new();
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            let label = match revision.filter(|r| !r.is_empty()) {
                Some(r) => format!("{n} Rev {r}"),
                None => n.to_string(),
            };
            markings.push(("kct:name", label));
        }
        if let Some(d) = date.filter(|d| !d.is_empty()) {
            markings.push(("kct:date", d.to_string()));
        }
        if markings.is_empty() {
            return result;
        }
        let (base_x, base_y) = self.get_marking_position();
        let mut y_offset = 0.0;
        for (tag, text) in markings {
            let existing_uuid = self.lookup_registry_uuid(tag);
            let existing = existing_uuid
                .as_deref()
                .and_then(|u| self.find_gr_text_by_uuid(u));
            if let Some(idx) = existing {
                let node = &self.doc.children[idx];
                if node.first_atom().is_some_and(|a| atom_text(a) == text) {
                    result.markings_skipped += 1;
                    result
                        .messages
                        .push(format!("Marking '{tag}' already exists, skipped"));
                    continue;
                }
                self.doc.children.remove(idx);
                self.drop_registry_entry(tag);
                result
                    .messages
                    .push(format!("Replaced stale marking '{tag}'"));
            } else if existing_uuid.is_some() {
                self.drop_registry_entry(tag);
            }
            let new_uuid = uuid::Uuid::new_v4().to_string();
            self.doc.push(gr_text_node(
                &text,
                base_x,
                base_y + y_offset,
                layer,
                font_size,
                font_thickness,
                &new_uuid,
            ));
            self.registry.push(MarkingEntry {
                uuid: new_uuid,
                tag: tag.to_string(),
                added_at: Some(utc_now_iso()),
            });
            result.markings_added += 1;
            result
                .messages
                .push(format!("Added marking '{tag}': {text}"));
            y_offset += font_size + 0.5;
        }
        result
    }

    /// Write the board and its marking sidecar.
    pub fn save(&self, output_path: Option<&Path>) -> Result<()> {
        let target = output_path.unwrap_or(&self.path);
        save_pcb(&self.doc, target)?;
        Self::write_sidecar(&Self::sidecar_for(target), &self.registry)
    }

    // ---------------------------------------------------------- sidecar I/O

    /// Missing/corrupt sidecars yield an empty registry.
    pub fn load_sidecar(path: &Path) -> Vec<MarkingEntry> {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let data: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "WARNING: Could not read marking sidecar {} ({e}); proceeding with empty registry.",
                    path.display()
                );
                return Vec::new();
            }
        };
        let Some(obj) = data.as_object() else {
            eprintln!(
                "WARNING: Sidecar {} has unexpected shape; ignoring.",
                path.display()
            );
            return Vec::new();
        };
        let Some(markings) = obj.get("markings").and_then(|m| m.as_array()) else {
            return Vec::new();
        };
        markings
            .iter()
            .filter_map(|e| {
                let e = e.as_object()?;
                Some(MarkingEntry {
                    uuid: e.get("uuid")?.as_str()?.to_string(),
                    tag: e.get("tag")?.as_str()?.to_string(),
                    added_at: e
                        .get("added_at")
                        .and_then(|a| a.as_str())
                        .map(str::to_string),
                })
            })
            .collect()
    }

    fn write_sidecar(path: &Path, registry: &[MarkingEntry]) -> Result<()> {
        if registry.is_empty() && !path.exists() {
            return Ok(());
        }
        let rows: Vec<Json> = registry
            .iter()
            .map(|e| {
                let mut m = Json::obj();
                m.set("uuid", e.uuid.as_str());
                m.set("tag", e.tag.as_str());
                if let Some(a) = &e.added_at {
                    m.set("added_at", a.as_str());
                }
                m
            })
            .collect();
        let mut payload = Json::obj();
        payload.set("version", SIDECAR_VERSION);
        payload.set("markings", Json::Arr(rows));
        let mut text = crate::pyjson::dumps_indent(&payload, 2);
        text.push('\n');
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::fsutil::atomic_write(path, text.as_bytes())
    }

    // ------------------------------------------------------------- registry

    fn lookup_registry_uuid(&self, tag: &str) -> Option<String> {
        self.registry
            .iter()
            .find(|e| e.tag == tag)
            .map(|e| e.uuid.clone())
    }

    fn drop_registry_entry(&mut self, tag: &str) {
        self.registry.retain(|e| e.tag != tag);
    }

    /// Top-level index of the `gr_text` whose `(uuid ...)` matches.
    fn find_gr_text_by_uuid(&self, target: &str) -> Option<usize> {
        if target.is_empty() {
            return None;
        }
        self.doc.children.iter().position(|c| {
            c.has_tag("gr_text")
                && c.find("uuid")
                    .and_then(|u| u.first_atom())
                    .is_some_and(|a| atom_text(a) == target)
        })
    }

    // ------------------------------------------------------------ internals

    /// Child index of the reference text (`fp_text reference` or KiCad 8+
    /// `property "Reference"`).
    pub fn find_ref_text(fp: &SExp) -> Option<usize> {
        let find = |tag: &str, key: &str| {
            fp.children
                .iter()
                .position(|c| !c.is_atom() && c.has_tag(tag) && first_atom_is(c, key))
        };
        find("fp_text", "reference").or_else(|| find("property", "Reference"))
    }

    pub fn extract_ref_name(fp: &SExp) -> String {
        for (tag, key) in [("fp_text", "reference"), ("property", "Reference")] {
            for c in &fp.children {
                if !c.is_atom() && c.has_tag(tag) {
                    let atoms: Vec<_> = c.atoms().collect();
                    if !atoms.is_empty() && atom_text(atoms[0]) == key && atoms.len() >= 2 {
                        return atom_text(atoms[1]);
                    }
                }
            }
        }
        "?".into()
    }

    pub fn is_silkscreen(text: &SExp) -> bool {
        text.find("layer")
            .and_then(|l| l.first_atom())
            .is_some_and(|a| SILKSCREEN_LAYERS.contains(&atom_text(a).as_str()))
    }

    pub fn is_hidden(text: &SExp) -> bool {
        if let Some(h) = text.find("hide") {
            match h.first_atom() {
                None => return true,
                Some(a) if atom_text(a) == "yes" => return true,
                _ => {}
            }
        }
        if text.children.iter().any(is_hide_atom) {
            return true;
        }
        if let Some(effects) = text.find("effects") {
            if effects.children.iter().any(is_hide_atom) || effects.find("hide").is_some() {
                return true;
            }
        }
        false
    }

    /// Remove all hide directives from a text node.
    pub fn unhide(text: &mut SExp) {
        remove_first_descendant(text, "hide");
        text.children.retain(|c| !is_hide_atom(c));
        if let Some(path) = first_descendant_path(text, "effects") {
            let effects = node_at_mut(text, &path);
            effects.children.retain(|c| !is_hide_atom(c));
            remove_first_descendant(effects, "hide");
        }
    }

    /// Just below the bottom-left of the Edge.Cuts outline, else (100, 115).
    pub fn get_marking_position(&self) -> (f64, f64) {
        let mut min_x: Option<f64> = None;
        let mut max_y: Option<f64> = None;
        for tag in ["gr_line", "gr_rect", "gr_arc"] {
            for node in self.doc.find_all(tag) {
                let Some(layer) = node.find("layer").filter(|l| !l.children.is_empty()) else {
                    continue;
                };
                if !layer
                    .first_atom()
                    .is_some_and(|a| atom_text(a) == "Edge.Cuts")
                {
                    continue;
                }
                for pos_tag in ["start", "end"] {
                    let Some(pos) = node.find(pos_tag) else {
                        continue;
                    };
                    let atoms: Vec<_> = pos.atoms().collect();
                    if atoms.len() >= 2 {
                        let (Some(x), Some(y)) = (atoms[0].as_f64(), atoms[1].as_f64()) else {
                            continue;
                        };
                        if min_x.is_none_or(|m| x < m) {
                            min_x = Some(x);
                        }
                        if max_y.is_none_or(|m| y > m) {
                            max_y = Some(y);
                        }
                    }
                }
            }
        }
        match (min_x, max_y) {
            (Some(x), Some(y)) => (x, y + 1.5),
            _ => (100.0, 115.0),
        }
    }
}

fn first_descendant_path(node: &SExp, tag: &str) -> Option<NodePath> {
    crate::drc::repair_silkscreen::descendant_paths(node, &|n| n.has_tag(tag))
        .into_iter()
        .next()
}

/// Python `node.remove(node.find(tag))`: `find` searches descendants but
/// `remove` only drops a direct child, so a nested match is left alone.
fn remove_first_descendant(node: &mut SExp, tag: &str) {
    if let Some(path) = first_descendant_path(node, tag) {
        if path.len() == 1 {
            node.children.remove(path[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gr_text_shape() {
        let s =
            gr_text_node("Hello", 10.0, 20.0, "F.SilkS", 1.0, 0.15, "test-uuid").to_kicad_string();
        assert!(s.contains("(at 10 20)"), "{s}");
        assert!(s.contains("(layer \"F.SilkS\")"), "{s}");
        assert!(s.contains("(uuid \"test-uuid\")"), "{s}");
        assert!(s.contains("(size 1 1)"), "{s}");
        assert!(s.contains("(thickness 0.15)"), "{s}");
    }

    #[test]
    fn iso_timestamp_shape() {
        let t = utc_now_iso();
        assert!(t.ends_with("+00:00") && t.as_bytes()[10] == b'T', "{t}");
    }
}
