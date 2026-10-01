//! Schematic / target-fab discovery for PCB-only entry points (minimal port
//! of `kicad_tools.sync.discover`, used by `kct check`).
//!
//! `project.kct` is read as plain YAML for the two keys consulted here
//! (`project.artifacts.schematic`, `requirements.manufacturing.target_fab`)
//! instead of the full spec model; a file that fails to parse is ignored,
//! matching upstream's defensive `except Exception`.

use std::path::{Path, PathBuf};

const PCB_STAGE_SUFFIXES: &[&str] = &["_routed", "_optimized", "_stitched", "_phase1"];

/// Strip one known pipeline-stage suffix from a PCB stem.
pub fn strip_stage_suffix(stem: &str) -> &str {
    for suffix in PCB_STAGE_SUFFIXES {
        if let Some(s) = stem.strip_suffix(suffix) {
            return s;
        }
    }
    stem
}

fn parent(p: &Path) -> PathBuf {
    p.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn project_kct(pcb_path: &Path) -> Option<PathBuf> {
    let dir = parent(pcb_path);
    let here = dir.join("project.kct");
    if here.exists() {
        return Some(here);
    }
    let up = parent(&dir).join("project.kct");
    up.exists().then_some(up)
}

fn load_yaml(path: &Path) -> Option<serde_yaml::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&text).ok()
}

fn yaml_str<'a>(v: &'a serde_yaml::Value, keys: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in keys {
        cur = cur.get(*k)?;
    }
    cur.as_str().filter(|s| !s.is_empty())
}

fn glob_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    })
    .into_iter()
    .flatten()
    .flatten()
    .map(|e| e.path())
    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
    .map(|p| {
        if dir.as_os_str().is_empty() {
            PathBuf::from(p.file_name().unwrap_or_default())
        } else {
            p
        }
    })
    .collect();
    out.sort();
    out
}

/// `resolve_schematic_for_pcb`.
pub fn resolve_schematic_for_pcb(pcb_path: &Path) -> Option<PathBuf> {
    let project_dir = parent(pcb_path);
    if let Some(kct) = project_kct(pcb_path) {
        if let Some(spec) = load_yaml(&kct) {
            if let Some(sch) = yaml_str(&spec, &["project", "artifacts", "schematic"]) {
                let p = parent(&kct).join(sch);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    let stem = pcb_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stripped = strip_stage_suffix(&stem);
    let mut candidates = vec![project_dir.join(format!("{stripped}.kicad_sch"))];
    if stripped != stem {
        candidates.push(pcb_path.with_extension("kicad_sch"));
    }
    if let Some(c) = candidates.into_iter().find(|c| c.exists()) {
        return Some(c);
    }
    let mut paired: Vec<PathBuf> = glob_ext(&project_dir, "kicad_pro")
        .into_iter()
        .map(|p| p.with_extension("kicad_sch"))
        .filter(|s| s.exists())
        .collect();
    paired.sort();
    paired.dedup();
    if paired.len() == 1 {
        return paired.pop();
    }
    if paired.len() > 1 {
        return None;
    }
    let mut all = glob_ext(&project_dir, "kicad_sch");
    if all.len() == 1 {
        return all.pop();
    }
    None
}

/// `resolve_target_fab_for_pcb`.
pub fn resolve_target_fab_for_pcb(pcb_path: &Path) -> Option<String> {
    let kct = project_kct(pcb_path)?;
    let spec = load_yaml(&kct)?;
    yaml_str(&spec, &["requirements", "manufacturing", "target_fab"]).map(str::to_string)
}
