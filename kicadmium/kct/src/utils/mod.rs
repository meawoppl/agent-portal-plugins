//! Utility helpers (port of `kicad_tools.utils`).
//!
//! Also hosts [`pyrepr`], the Python `str()`/`repr()` formatting helpers the
//! port uses wherever upstream interpolates values into user-facing text
//! (`f"{value!r}"`), so messages stay byte-compatible with kicad-tools.

pub mod pyfmt;
pub mod pymath;
pub mod pyrepr;
pub mod scoring;
pub mod stdsort;

use std::path::Path;

pub use scoring::{
    adjust_confidence, calculate_string_confidence, combine_confidences, CombineMethod,
    ConfidenceLevel, MatchResult, StringConfidenceOptions,
};

/// Ensure the parent directory of `path` exists (creating ancestors), and
/// return `path` unchanged so calls can be chained.
pub fn ensure_parent_dir(path: &Path) -> std::io::Result<&Path> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    Ok(path)
}
