//! Disposable project snapshot for native checks that refill/save board zones.
use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub(crate) fn copy_project(root: &Path, source: &Path, scratch: &Path) -> Result<PathBuf> {
    for entry in walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        !matches!(
            e.file_name().to_str(),
            Some(".git" | "target" | "node_modules")
        )
    }) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if !matches!(
            entry.path().extension().and_then(|s| s.to_str()),
            Some("kicad_pcb" | "kicad_sch" | "kicad_pro" | "kicad_dru" | "kicad_sym")
        ) {
            continue;
        }
        let dest = scratch.join(entry.path().strip_prefix(root)?);
        fs::create_dir_all(dest.parent().context("source parent")?)?;
        fs::copy(entry.path(), dest)?;
    }
    let board = scratch.join(
        source
            .strip_prefix(root)
            .context("board must be inside project root")?,
    );
    if !board.is_file() {
        bail!("board was not copied into check workspace");
    }
    Ok(board)
}
pub(crate) fn save_fills(source: &Path, checked: &Path, original: &[u8]) -> Result<()> {
    if fs::read(source)? != original {
        bail!("board changed during DRC; refusing to overwrite concurrent edits");
    }
    let parent = source.parent().context("board parent")?;
    let staged = tempfile::NamedTempFile::new_in(parent)?;
    fs::copy(checked, staged.path())?;
    staged.as_file().sync_all()?;
    staged.persist(source).context("save filled board")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_preserves_hierarchy_and_never_rewrites_source() {
        let project = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join("sheets")).unwrap();
        for name in [
            "board.kicad_pcb",
            "board.kicad_pro",
            "board.kicad_dru",
            "board.kicad_sch",
            "sheets/child.kicad_sch",
        ] {
            fs::write(project.path().join(name), name).unwrap();
        }
        let source = project.path().join("board.kicad_pcb");
        let checked = copy_project(project.path(), &source, scratch.path()).unwrap();
        assert!(scratch.path().join("sheets/child.kicad_sch").is_file());
        assert!(scratch.path().join("board.kicad_dru").is_file());
        fs::write(&checked, "filled").unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"board.kicad_pcb");
        save_fills(&source, &checked, b"board.kicad_pcb").unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"filled");
        fs::write(&source, "concurrent edit").unwrap();
        assert!(save_fills(&source, &checked, b"filled").is_err());
        assert_eq!(fs::read(&source).unwrap(), b"concurrent edit");
    }
}
