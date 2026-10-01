//! Shared sidecar-filename resolution (port of `kicad_tools.sidecars`,
//! Issue #4634).
//!
//! Single source of truth for which net-class-map sidecar names are probed,
//! and in what order, within a directory. Which *directories* are searched
//! stays the caller's choice.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Bare, board-agnostic sidecar name written by `kct route`.
pub const NET_CLASS_MAP_SIDECAR_BASENAME: &str = "net_class_map.json";

/// Names to probe in one directory: `<pcb_stem>.net_class_map.json` first
/// (exact stem, no globbing), then the bare name. An empty stem yields only
/// the bare name.
pub fn net_class_map_sidecar_names(pcb_stem: &str) -> Vec<String> {
    if pcb_stem.is_empty() {
        return vec![NET_CLASS_MAP_SIDECAR_BASENAME.to_string()];
    }
    vec![
        format!("{pcb_stem}.{NET_CLASS_MAP_SIDECAR_BASENAME}"),
        NET_CLASS_MAP_SIDECAR_BASENAME.to_string(),
    ]
}

/// `Path.stem`.
fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `kct check`'s probe: `pcb_dir`, `pcb_dir/output`, `pcb_dir/../output`,
/// each with both names, de-duplicated, nearest first.
pub fn net_class_map_sidecar_candidates(pcb_path: impl AsRef<Path>) -> Vec<PathBuf> {
    let pcb_path = pcb_path.as_ref();
    let pcb_dir = pcb_path.parent().unwrap_or(Path::new("")).to_path_buf();
    let parent = pcb_dir.parent().unwrap_or(Path::new("")).to_path_buf();
    let directories = [
        pcb_dir.clone(),
        pcb_dir.join("output"),
        parent.join("output"),
    ];
    candidate_paths(&directories, &stem(pcb_path))
}

/// First existing sidecar over caller-chosen directories (nearest first).
pub fn first_existing_net_class_map_sidecar<I, P>(directories: I, pcb_stem: &str) -> Option<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let dirs: Vec<PathBuf> = directories
        .into_iter()
        .map(|d| d.as_ref().to_path_buf())
        .collect();
    candidate_paths(&dirs, pcb_stem)
        .into_iter()
        .find(|c| c.is_file())
}

fn candidate_paths(directories: &[PathBuf], pcb_stem: &str) -> Vec<PathBuf> {
    let names = net_class_map_sidecar_names(pcb_stem);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in directories {
        for name in &names {
            let candidate = normalize(&dir.join(name));
            if seen.insert(candidate.clone()) {
                out.push(candidate);
            }
        }
    }
    out
}

/// Lexical normalization matching `pathlib` equality (drops `.` segments).
fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}
