//! Shared atomic-write primitive (port of `kicad_tools.core.atomic_write`).
//!
//! Delegates to [`crate::fsutil::atomic_write`] (sibling temp file, fsync,
//! rename): a crash mid-write leaves either the old or the new content,
//! never a torn file.

use std::path::Path;

pub use crate::fsutil::atomic_write;

/// Atomically write UTF-8 `content` to `path` (`atomic_write_text`). Bytes
/// are written verbatim (no newline translation).
pub fn atomic_write_text(path: impl AsRef<Path>, content: &str) -> crate::Result<()> {
    atomic_write(path.as_ref(), content.as_bytes())
}
