//! Declared branch-specific current paths (partial port of
//! `kicad_tools.router.current_paths`): the sidecar probe and loader that
//! `kct check` uses. Path resolution / ampacity evaluation of declared
//! branches is not ported yet (see `validate::rules::path_ampacity`).

use std::path::{Path, PathBuf};

use crate::pyjson::{self, Json};

pub const CURRENT_PATHS_SIDECAR_BASENAME: &str = "current_paths.json";

/// One declared current path (the raw sidecar entry is retained).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurrentPathSpec {
    pub name: String,
    pub net_name: String,
    pub continuous_a: f64,
    pub raw: Json,
}

impl CurrentPathSpec {
    pub fn from_dict(data: &Json) -> Result<Self, String> {
        if !matches!(data, Json::Obj(_)) {
            return Err(format!(
                "current-path entry must be an object, got {}",
                data.py_type_name()
            ));
        }
        let s = |k: &str| -> Result<String, String> {
            data.get(k)
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("current-path entry missing non-empty '{k}'"))
        };
        Ok(CurrentPathSpec {
            name: s("name")?,
            net_name: s("net")?,
            continuous_a: data
                .get("continuous_a")
                .and_then(Json::as_f64)
                .ok_or_else(|| "current-path entry missing numeric 'continuous_a'".to_string())?,
            raw: data.clone(),
        })
    }
}

fn names(stem: &str) -> Vec<String> {
    if stem.is_empty() {
        vec![CURRENT_PATHS_SIDECAR_BASENAME.into()]
    } else {
        vec![
            format!("{stem}.{CURRENT_PATHS_SIDECAR_BASENAME}"),
            CURRENT_PATHS_SIDECAR_BASENAME.into(),
        ]
    }
}

/// Board dir, `output/`, `../output/` crossed with stem-keyed-then-bare
/// names, de-duplicated.
pub fn current_paths_sidecar_candidates(pcb_path: &Path) -> Vec<PathBuf> {
    let dir = pcb_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let up = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = pcb_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out: Vec<PathBuf> = Vec::new();
    for d in [dir.clone(), dir.join("output"), up.join("output")] {
        for n in names(&stem) {
            let c = d.join(n);
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

pub fn discover_current_paths_sidecar(pcb_path: &Path) -> Option<PathBuf> {
    current_paths_sidecar_candidates(pcb_path)
        .into_iter()
        .find(|c| c.is_file())
}

pub fn parse_current_path_specs(data: &Json) -> Result<Vec<CurrentPathSpec>, String> {
    let raw = match data {
        Json::Obj(_) => data.get("paths").cloned().unwrap_or(Json::Arr(vec![])),
        Json::Arr(_) => data.clone(),
        other => {
            return Err(format!(
                "current-paths sidecar must be a list or a {{'paths': [...]}} object, got {}",
                other.py_type_name()
            ))
        }
    };
    let Json::Arr(list) = raw else {
        return Err("current-paths sidecar 'paths' key must be a list".into());
    };
    list.iter().map(CurrentPathSpec::from_dict).collect()
}

pub fn load_current_path_specs(path: &Path) -> Result<Vec<CurrentPathSpec>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = pyjson::loads(&text).map_err(|e| e.to_string())?;
    parse_current_path_specs(&data)
}
