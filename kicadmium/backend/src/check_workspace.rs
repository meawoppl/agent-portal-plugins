//! Disposable project snapshot for native checks that refill/save board zones.
use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub(crate) fn copy_project(root: &Path, source: &Path, scratch: &Path) -> Result<PathBuf> {
    for entry in walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            !e.path().starts_with(scratch)
                && !matches!(
                    e.file_name().to_str(),
                    Some(".git" | "target" | "node_modules")
                )
        })
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let library_table = matches!(
            entry.file_name().to_str(),
            Some("fp-lib-table" | "sym-lib-table")
        );
        let library_asset = entry.path().strip_prefix(root)?.ancestors().any(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("pretty" | "3dshapes")
            ) || p.file_name().and_then(|s| s.to_str()) == Some("3dmodels")
        });
        if !library_table
            && !library_asset
            && !matches!(
                entry.path().extension().and_then(|s| s.to_str()),
                Some(
                    "kicad_pcb"
                        | "kicad_sch"
                        | "kicad_pro"
                        | "kicad_dru"
                        | "kicad_sym"
                        | "kicad_mod"
                        | "step"
                        | "stp"
                        | "wrl"
                        | "wrz"
                )
            )
        {
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

#[cfg(test)]
mod library_tests {
    use super::*;

    #[test]
    fn snapshot_keeps_project_tables_and_local_library_assets() {
        let project = tempfile::tempdir().unwrap();
        let scratch = project.path().join("scratch");
        let files = [
            "boards/controller.kicad_pcb",
            "boards/fp-lib-table",
            "boards/sym-lib-table",
            "boards/local.pretty/tab.kicad_mod",
            "boards/3dmodels/tab.step",
            "boards/custom.3dshapes/tab.wrl",
            "symbols/custom.kicad_sym",
        ];
        for name in files {
            let path = project.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
        }
        fs::write(project.path().join("boards/fp-lib-table"), r#"(fp_lib_table (lib (name "local") (type "KiCad") (uri "${KIPRJMOD}/local.pretty") (options "") (descr "")))"#).unwrap();
        let source = project.path().join(files[0]);
        copy_project(project.path(), &source, &scratch).unwrap();
        for name in files {
            assert_eq!(
                fs::read(scratch.join(name)).unwrap(),
                fs::read(project.path().join(name)).unwrap()
            );
        }
        assert!(!scratch.join("scratch").exists());
        fs::write(scratch.join("boards/local.pretty/tab.kicad_mod"), "changed").unwrap();
        assert_eq!(
            fs::read(project.path().join("boards/local.pretty/tab.kicad_mod")).unwrap(),
            b"boards/local.pretty/tab.kicad_mod"
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_project_library_is_materialized_in_snapshot() {
        let project = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        fs::write(project.path().join("board.kicad_pcb"), "board").unwrap();
        fs::write(library.path().join("part.kicad_mod"), "footprint").unwrap();
        std::os::unix::fs::symlink(library.path(), project.path().join("local.pretty")).unwrap();
        copy_project(
            project.path(),
            &project.path().join("board.kicad_pcb"),
            scratch.path(),
        )
        .unwrap();
        assert_eq!(
            fs::read(scratch.path().join("local.pretty/part.kicad_mod")).unwrap(),
            b"footprint"
        );
        assert!(!fs::symlink_metadata(scratch.path().join("local.pretty"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}

#[cfg(test)]
mod native_library_tests {
    use super::*;

    #[test]
    #[ignore = "requires KiCad and KICADMIUM_DRC_FIXTURE pointing to a project board"]
    fn native_drc_library_findings_match_source_project() {
        let source =
            PathBuf::from(std::env::var_os("KICADMIUM_DRC_FIXTURE").expect("board fixture"));
        let original = fs::read(&source).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let copied = copy_project(
            source.parent().unwrap(),
            &source,
            &scratch.path().join("project"),
        )
        .unwrap();
        let check = |board: &Path, name: &str| {
            let output = scratch.path().join(name);
            let result = std::process::Command::new("kicad-cli")
                .args([
                    "pcb",
                    "drc",
                    "--schematic-parity",
                    "--format",
                    "json",
                    "--output",
                ])
                .arg(&output)
                .arg(board)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let report: serde_json::Value =
                serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            report["violations"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v["type"] == "lib_footprint_issues")
                .count()
        };
        let source_count = check(&source, "source.json");
        let scratch_count = check(&copied, "scratch.json");
        println!("library findings: source={source_count}, scratch={scratch_count}");
        assert_eq!(scratch_count, source_count);
        assert_eq!(fs::read(source).unwrap(), original);
    }
}
