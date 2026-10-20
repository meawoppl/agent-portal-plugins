//! `GET /api/kicad/schematic`: the project's schematic as a `vector_view::Scene`.
//!
//! Geometry comes from pastebom's filesystem-free `schematic-extract`. This
//! module owns everything that touches the disk: which files the extractor
//! may see (only sheets that resolve inside the project root, symlinks
//! included), the revision-stable read, and the `revision` metadata.

use crate::{
    current_source_revision, pick_project_file, rel, revision, selected_project, AppError,
    AppState, ProjectContext, ProjectQuery,
};
use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use schematic_extract::{kicad_schema::sexp, ExtractOptions};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};
use vector_view::{Prop, Scene};

/// Label drawn in the bottom-left title-block cell of every page.
const FRAME_LABEL: &str = "kicadmium";

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/kicad/schematic", get(endpoint))
}

async fn endpoint(
    State(state): State<AppState>,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<Scene>, AppError> {
    let p = selected_project(&state, q.project.as_deref())?;
    Ok(Json(
        tokio::task::spawn_blocking(move || build_scene(&p)).await??,
    ))
}

fn build_scene(project: &ProjectContext) -> Result<Scene> {
    let root = pick_project_file(project, "kicad_sch")?
        .ok_or_else(|| anyhow!("project {} has no .kicad_sch", project.id))?;
    let options = ExtractOptions {
        frame_label: Some(FRAME_LABEL.into()),
        ..ExtractOptions::default()
    };
    for _ in 0..4 {
        let rev = current_source_revision(project)?;
        let (root_key, files) = schematic_files(project, &root)?;
        let mut scene = schematic_extract::parse_project_with(&root_key, &files, &options)?;
        if current_source_revision(project)? == rev {
            // Keep the metadata order clients have always seen:
            // page entries, then `revision`, then `root`.
            let at = scene
                .meta
                .iter()
                .position(|p| p.key == "root")
                .unwrap_or(scene.meta.len());
            scene.meta.insert(at, Prop::new("revision", rev));
            return Ok(scene);
        }
    }
    Err(anyhow!("schematic kept changing while it was read"))
}

/// The in-memory project handed to the extractor: project-relative keys to
/// file contents. It holds the schematic source set the revision hashes, plus
/// every sheet reachable from `root` (a sheet can live in a nested project
/// that the source set excludes). A file is only included when its canonical
/// path is inside the project root, so `..` paths and symlinks that escape
/// the project are never read; the extractor reports them as missing sheets.
fn schematic_files(
    project: &ProjectContext,
    root: &Path,
) -> Result<(String, HashMap<String, Vec<u8>>)> {
    let root = root
        .canonicalize()
        .with_context(|| format!("reading {}", root.display()))?;
    if !root.starts_with(&project.root) {
        return Err(anyhow!(
            "schematic {} is outside the project",
            root.display()
        ));
    }
    let root_key = rel(&project.root, &root)?;
    let mut files = HashMap::new();
    for (key, path) in &revision::cached_sources(project).sch {
        // `read_confined` is the safety check; keys outside the project
        // (`@repo/...`) never pass it.
        if key.ends_with(".kicad_sch") {
            if let Some(data) = read_confined(project, path) {
                files.insert(key.clone(), data);
            }
        }
    }
    let mut queue = VecDeque::from([root_key.clone()]);
    let mut visited = HashSet::new();
    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        if !files.contains_key(&key) {
            match read_confined(project, &project.root.join(&key)) {
                Some(data) => {
                    files.insert(key.clone(), data);
                }
                None if key == root_key => {
                    return Err(anyhow!("reading {}", root.display()));
                }
                None => continue,
            }
        }
        let parent = key.rsplit_once('/').map_or("", |(dir, _)| dir);
        queue.extend(
            child_sheets(&files[&key])
                .iter()
                .filter_map(|file| resolve(parent, file)),
        );
    }
    Ok((root_key, files))
}

fn read_confined(project: &ProjectContext, path: &Path) -> Option<Vec<u8>> {
    let canonical: PathBuf = path.canonicalize().ok()?;
    if !canonical.starts_with(&project.root) || !canonical.is_file() {
        return None;
    }
    std::fs::read(canonical).ok()
}

/// `Sheetfile` of every direct child sheet. Unparsable files yield none; the
/// extractor reports their errors.
fn child_sheets(data: &[u8]) -> Vec<String> {
    let Some(tree) = std::str::from_utf8(data)
        .ok()
        .and_then(|text| sexp::parse(text).ok())
    else {
        return Vec::new();
    };
    tree.children_named("sheet")
        .filter_map(|sheet| sheet.property("Sheetfile").map(str::to_owned))
        .collect()
}

/// Resolve a sheet file lexically against its parent's directory, the way
/// the extractor does; `None` for absolute paths or paths above the root.
fn resolve(parent: &str, file: &str) -> Option<String> {
    // Same rules as the extractor: absolute, drive and backslash paths are
    // not project members (it reports them as missing sheets).
    if file.starts_with('/') || file.contains([':', '\\']) {
        return None;
    }
    let mut parts: Vec<&str> = parent.split('/').filter(|p| !p.is_empty()).collect();
    for part in file.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(name: &str, file: &str) -> String {
        format!(
            r#"(sheet (at 10 10) (size 20 20) (uuid "{name}-uuid")
                (property "Sheetname" "{name}" (at 10 9 0))
                (property "Sheetfile" "{file}" (at 10 31 0)))"#
        )
    }

    fn schematic(uuid: &str, sheets: &[String]) -> String {
        format!(
            r#"(kicad_sch (version 20231120) (generator "test") (uuid "{uuid}") (paper "A4") {})"#,
            sheets.join(" ")
        )
    }

    /// A workspace at `outer` with one project rooted at `outer/board`.
    fn project(outer: &Path) -> ProjectContext {
        std::fs::write(
            outer.join(".kicad-pcb.json"),
            r#"{"version": 1, "projects": [{"id": "t", "root": "board",
                "kicad": {"schematic": "root.kicad_sch"}}]}"#,
        )
        .unwrap();
        crate::project_contexts(outer).unwrap().remove(0)
    }

    #[test]
    fn resolves_children_lexically_inside_the_project() {
        assert_eq!(resolve("", "a.kicad_sch").as_deref(), Some("a.kicad_sch"));
        assert_eq!(
            resolve("sub", "../b.kicad_sch").as_deref(),
            Some("b.kicad_sch")
        );
        assert_eq!(resolve("", "../out.kicad_sch"), None);
        assert_eq!(resolve("", "/etc/x.kicad_sch"), None);
        assert_eq!(resolve("", "c:/x.kicad_sch"), None);
    }

    #[test]
    fn hierarchy_renders_and_escapes_and_recursion_become_warnings() {
        let outer = tempfile::tempdir().unwrap();
        let outer = outer.path().canonicalize().unwrap();
        let dir = outer.join("board");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(outer.join("secret.kicad_sch"), schematic("secret", &[])).unwrap();
        std::fs::write(
            dir.join("root.kicad_sch"),
            schematic(
                "root",
                &[
                    sheet("Child", "sub/child.kicad_sch"),
                    sheet("Up", "../secret.kicad_sch"),
                    sheet("Again", "root.kicad_sch"),
                    sheet("Link", "link.kicad_sch"),
                ],
            ),
        )
        .unwrap();
        std::fs::write(dir.join("sub/child.kicad_sch"), schematic("child", &[])).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outer.join("secret.kicad_sch"), dir.join("link.kicad_sch"))
            .unwrap();

        let project = project(&outer);
        let (root, files) = schematic_files(&project, &dir.join("root.kicad_sch")).unwrap();
        assert_eq!(root, "root.kicad_sch");
        assert!(files.contains_key("sub/child.kicad_sch"));
        assert!(!files.contains_key("link.kicad_sch"), "symlink escaped");
        assert!(files.keys().all(|k| !k.contains("secret")));

        let scene = build_scene(&project).unwrap();
        let meta: Vec<_> = scene.meta.iter().map(|p| p.key.as_str()).collect();
        assert!(meta.contains(&"page:/Child:file"));
        // Escaping, recursive and symlinked-out sheets are skipped with a
        // warning; the rest of the hierarchy still renders.
        let warnings = vector_view::schematic::warnings(&scene);
        assert!(
            warnings.iter().any(|w| w.contains("../secret.kicad_sch")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("recursive")),
            "{warnings:?}"
        );
        #[cfg(unix)]
        assert!(
            warnings.iter().any(|w| w.contains("link.kicad_sch")),
            "{warnings:?}"
        );
        assert!(warnings.iter().all(|w| !w.contains("outer")));
        // `revision` stays immediately before `root`, after the pages.
        let (r, root) = (
            meta.iter().position(|k| *k == "revision").unwrap(),
            meta.iter().position(|k| *k == "root").unwrap(),
        );
        assert_eq!(r + 1, root);
        assert_eq!(
            scene.meta[r].value,
            current_source_revision(&project).unwrap()
        );
    }
}
