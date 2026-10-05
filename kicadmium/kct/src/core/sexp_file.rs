//! Typed load/save of KiCad s-expression files (port of
//! `kicad_tools.core.sexp_file`).
//!
//! Errors are `anyhow` errors wrapping typed causes so callers can
//! `downcast_ref`: [`KiCadToolsError`] (`FileNotFound` / `FileFormat`),
//! [`KiCadLockError`], [`WriteVerificationError`], or I/O errors.
//! Savers run the advisory [`check_kicad_lock`] and write atomically, except
//! `save_symbol_lib`, which (as upstream) writes directly.

use std::fmt;
use std::path::Path;

use anyhow::Context;
use serde_json::json;

use super::atomic_write::atomic_write_text;
use super::kicad_lock::check_kicad_lock;
use crate::exceptions::KiCadToolsError;
use crate::sexp::{parse, SExp};
use crate::Result;

/// Python `read_text()` universal-newline decoding.
fn read_text(path: &Path) -> Result<String> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    })
}

fn not_found(message: &str, path: &Path, extension: &str) -> anyhow::Error {
    KiCadToolsError::file_not_found(message)
        .with_context("file", path.display().to_string())
        .with_suggestions([
            "Check that the file path is correct".to_string(),
            format!("Ensure the file has a {extension} extension"),
        ])
        .into()
}

fn wrong_type(
    message: &str,
    path: Option<&Path>,
    expected: &str,
    got: Option<&str>,
) -> anyhow::Error {
    let mut err = KiCadToolsError::file_format(message);
    if let Some(path) = path {
        err = err.with_context("file", path.display().to_string());
    }
    err = err
        .with_context("expected", expected)
        .with_context("got", json!(got));
    if path.is_some() {
        err = err.with_suggestion("This file appears to be a different KiCad file type");
    }
    err.into()
}

fn load_tagged(
    path: &Path,
    missing: &str,
    extension: &str,
    tags: &[&str],
    wrong: &str,
    expected: &str,
    raw: bool,
) -> Result<SExp> {
    if !path.exists() {
        return Err(not_found(missing, path, extension));
    }
    let text = if raw {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        read_text(path)?
    };
    let sexp = parse(&text).with_context(|| format!("parsing {}", path.display()))?;
    if !tags.iter().any(|t| sexp.has_tag(t)) {
        return Err(wrong_type(wrong, Some(path), expected, sexp.tag()));
    }
    Ok(sexp)
}

fn require_tag(sexp: &SExp, tags: &[&str], message: &str, expected: &str) -> Result<()> {
    if tags.iter().any(|t| sexp.has_tag(t)) {
        Ok(())
    } else {
        Err(wrong_type(message, None, expected, sexp.tag()))
    }
}

/// Upstream `serialize_sexp(sexp)` (canonical KiCad formatting, no trailing
/// newline).
pub fn serialize_sexp(sexp: &SExp) -> String {
    sexp.to_kicad_string()
}

/// Load a `.kicad_sch` (root tag `kicad_sch`).
pub fn load_schematic(path: impl AsRef<Path>) -> Result<SExp> {
    load_tagged(
        path.as_ref(),
        "Schematic file not found",
        ".kicad_sch",
        &["kicad_sch"],
        "Not a KiCad schematic file",
        "kicad_sch",
        false,
    )
}

/// Save a schematic (lock check + atomic write).
pub fn save_schematic(sexp: &SExp, path: impl AsRef<Path>) -> Result<()> {
    require_tag(sexp, &["kicad_sch"], "Not a KiCad schematic", "kicad_sch")?;
    let path = path.as_ref();
    let text = serialize_sexp(sexp);
    check_kicad_lock(path, None)?;
    atomic_write_text(path, &text)
}

/// Load a `.kicad_sym` (root tag `kicad_symbol_lib`).
pub fn load_symbol_lib(path: impl AsRef<Path>) -> Result<SExp> {
    load_tagged(
        path.as_ref(),
        "Symbol library not found",
        ".kicad_sym",
        &["kicad_symbol_lib"],
        "Not a KiCad symbol library",
        "kicad_symbol_lib",
        false,
    )
}

/// Save a symbol library (plain write, as upstream).
pub fn save_symbol_lib(sexp: &SExp, path: impl AsRef<Path>) -> Result<()> {
    require_tag(
        sexp,
        &["kicad_symbol_lib"],
        "Not a KiCad symbol library",
        "kicad_symbol_lib",
    )?;
    let path = path.as_ref();
    std::fs::write(path, serialize_sexp(sexp))
        .with_context(|| format!("writing {}", path.display()))
}

/// Load a `.kicad_pcb` (root tag `kicad_pcb`), keeping line endings intact
/// so untouched copper-arc source text round-trips byte-exact.
pub fn load_pcb(path: impl AsRef<Path>) -> Result<SExp> {
    load_tagged(
        path.as_ref(),
        "PCB file not found",
        ".kicad_pcb",
        &["kicad_pcb"],
        "Not a KiCad PCB file",
        "kicad_pcb",
        true,
    )
}

/// Save a PCB, replaying untouched copper-arc source text.
pub fn save_pcb(sexp: &SExp, path: impl AsRef<Path>) -> Result<()> {
    require_tag(sexp, &["kicad_pcb"], "Not a KiCad PCB", "kicad_pcb")?;
    let path = path.as_ref();
    let text = sexp.to_kicad_string_preserving();
    check_kicad_lock(path, None)?;
    atomic_write_text(path, &text)
}

/// Post-write verification found missing structures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteVerificationError(pub String);

impl fmt::Display for WriteVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WriteVerificationError {}

/// Re-read a PCB and require at least the given numbers of `zone`, `via`,
/// and `segment` nodes (catches silent persistence failures).
pub fn verify_pcb_write(
    path: impl AsRef<Path>,
    expected_zones: usize,
    expected_vias: usize,
    expected_segments: usize,
) -> Result<()> {
    let path = path.as_ref();
    let sexp = load_pcb(path)?;
    let mut errors = Vec::new();
    for (tag, expected, label) in [
        ("zone", expected_zones, "zone(s)"),
        ("via", expected_vias, "via(s)"),
        ("segment", expected_segments, "segment(s)"),
    ] {
        if expected > 0 {
            let actual = sexp.find_all(tag).count();
            if actual < expected {
                errors.push(format!(
                    "Expected at least {expected} {label}, found {actual}"
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(WriteVerificationError(format!(
            "Post-write verification failed for {}: {}",
            path.display(),
            errors.join("; ")
        ))
        .into())
    }
}

/// Load a `.kicad_mod` (KiCad 5 `module` or KiCad 6+ `footprint`).
pub fn load_footprint(path: impl AsRef<Path>) -> Result<SExp> {
    load_tagged(
        path.as_ref(),
        "Footprint file not found",
        ".kicad_mod",
        &["module", "footprint"],
        "Not a KiCad footprint file",
        "module or footprint",
        false,
    )
}

/// Save a footprint (lock check + atomic write).
pub fn save_footprint(sexp: &SExp, path: impl AsRef<Path>) -> Result<()> {
    require_tag(
        sexp,
        &["module", "footprint"],
        "Not a KiCad footprint",
        "module or footprint",
    )?;
    let path = path.as_ref();
    let text = serialize_sexp(sexp);
    check_kicad_lock(path, None)?;
    atomic_write_text(path, &text)
}

/// Load a `.kicad_dru`: the file's top-level expressions wrapped in a
/// `(design_rules ...)` container; the first must be `(version N)`.
pub fn load_design_rules(path: impl AsRef<Path>) -> Result<SExp> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(not_found("Design rules file not found", path, ".kicad_dru"));
    }
    let text = read_text(path)?;
    let sexp = parse(&format!("(design_rules {text})"))
        .with_context(|| format!("parsing {}", path.display()))?;
    let Some(first) = sexp.children.first() else {
        return Err(KiCadToolsError::file_format("Empty design rules file")
            .with_context("file", path.display().to_string())
            .with_suggestion("Design rules file should contain at least a version")
            .into());
    };
    if !first.has_tag("version") {
        let got = match &first.value {
            Some(v) if first.is_atom() => v.to_string(),
            _ => first.to_kicad_string(),
        };
        return Err(KiCadToolsError::file_format("Invalid design rules file")
            .with_context("file", path.display().to_string())
            .with_context("expected", "version")
            .with_context("got", got)
            .with_suggestions([
                "Design rules file should start with (version N)",
                "Use 'kct mfr export-dru' to generate a valid file",
            ])
            .into());
    }
    Ok(sexp)
}

/// Save a `(design_rules ...)` container: each child serialized, joined by
/// newlines, without the wrapper.
pub fn save_design_rules(sexp: &SExp, path: impl AsRef<Path>) -> Result<()> {
    require_tag(
        sexp,
        &["design_rules"],
        "Not a design rules container",
        "design_rules",
    )?;
    let path = path.as_ref();
    let lines: Vec<String> = sexp.children.iter().map(serialize_sexp).collect();
    check_kicad_lock(path, None)?;
    atomic_write_text(path, &lines.join("\n"))
}
