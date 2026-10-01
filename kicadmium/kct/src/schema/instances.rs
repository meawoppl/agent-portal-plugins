//! Project instance metadata for placed symbols (port of
//! `kicad_tools.schema.instances`): project name and hierarchical
//! `(instances (project ... (path ...)))` paths.

use std::path::{Path, PathBuf};

use super::hierarchy::build_hierarchy;

fn pro_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "kicad_pro"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Prefer the `.kicad_pro` whose stem matches the schematic, else the first sorted.
fn select_pro_file(pro_files: &[PathBuf], schematic_path: &Path) -> PathBuf {
    let stem = schematic_path.file_stem();
    pro_files
        .iter()
        .find(|p| p.file_stem() == stem)
        .unwrap_or(&pro_files[0])
        .clone()
}

fn resolve(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    })
}

fn stem_string(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Project name from the nearest `.kicad_pro` walking up from the
/// schematic; falls back to the schematic stem.
pub fn find_project_name(schematic_path: &Path) -> String {
    let resolved = resolve(schematic_path);
    for dir in resolved.parent().into_iter().flat_map(Path::ancestors) {
        let files = pro_files(dir);
        if !files.is_empty() {
            return stem_string(&select_pro_file(&files, schematic_path));
        }
    }
    stem_string(schematic_path)
}

/// Hierarchical instance path: `/<sch_uuid>` for a root schematic, else
/// the UUID chain `/root/.../sheet` found by walking the project hierarchy.
pub fn build_instance_path(schematic_path: &Path, sch_uuid: &str) -> String {
    let resolved = resolve(schematic_path);
    let mut root_sch: Option<PathBuf> = None;
    for dir in resolved.parent().into_iter().flat_map(Path::ancestors) {
        let files = pro_files(dir);
        if !files.is_empty() {
            let candidate = select_pro_file(&files, schematic_path).with_extension("kicad_sch");
            if candidate.exists() {
                root_sch = Some(candidate);
            }
            break;
        }
    }
    let fallback = format!("/{sch_uuid}");
    let Some(root_sch) = root_sch else {
        return fallback;
    };
    if resolve(&root_sch) == resolved {
        return fallback;
    }
    let root = build_hierarchy(&root_sch);
    for node in root.all_nodes() {
        if resolve(Path::new(&node.path)) == resolved {
            return format!("/{}", node.uuid_path().join("/"));
        }
    }
    fallback
}
