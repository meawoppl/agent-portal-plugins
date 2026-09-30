//! File helpers shared by mutating commands (port of `core.atomic_write`).

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Write via a sibling temp file + rename so readers (and KiCad) never see a
/// half-written design file.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.kct-tmp-{}", std::process::id()));
    {
        let mut file =
            std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}
