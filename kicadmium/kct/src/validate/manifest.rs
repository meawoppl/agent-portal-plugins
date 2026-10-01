//! Manufacturing-bundle freshness check for `kct check`'s meta rollup (port
//! of `check_cmd._manifest_subcheck`'s verification body plus
//! `export.manufacturing.verify_manifest`).

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::pyjson::{self, Json};

static SHA_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").unwrap());

fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn resolve(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

fn rglob(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.file_name().and_then(|n| n.to_str()) == Some(name) {
            out.push(p.clone());
        }
        if p.is_dir() {
            rglob(&p, name, out);
        }
    }
}

/// `verify_manifest(manifest_path)`: mismatch descriptions (empty = ok).
pub fn verify_manifest(manifest_path: &Path) -> Result<Vec<String>, String> {
    let bundle = manifest_path.parent().unwrap_or(Path::new("."));
    let text = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let manifest = pyjson::loads(&text).map_err(|e| {
        crate::utils::pyjsondecode::json_decode_error(&text).unwrap_or_else(|| e.to_string())
    })?;
    let own = manifest_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let mut problems = Vec::new();
    let Some(Json::Obj(files)) = manifest.get("files") else {
        return Ok(problems);
    };
    for (name, info) in files {
        if name == own {
            continue;
        }
        let mut fpath = bundle.join(name);
        if !fpath.exists() {
            let mut cands = Vec::new();
            rglob(bundle, name, &mut cands);
            cands.sort();
            let Some(first) = cands.into_iter().next() else {
                problems.push(format!("{name}: listed in manifest but not found on disk"));
                continue;
            };
            fpath = first;
        }
        let bytes = std::fs::read(&fpath).map_err(|e| e.to_string())?;
        let actual_sha = sha256_hex(&bytes);
        let actual_size = bytes.len() as i64;
        if let Some(sha) = info.get("sha256").filter(|v| !v.is_null()) {
            if sha.as_str() != Some(actual_sha.as_str()) {
                problems.push(format!(
                    "{name}: sha256 mismatch (manifest {}, disk {actual_sha})",
                    match sha {
                        Json::Str(s) => s.clone(),
                        other => other.py_repr(),
                    }
                ));
            }
        }
        if let Some(size) = info.get("size").filter(|v| !v.is_null()) {
            if size.as_f64() != Some(actual_size as f64) {
                problems.push(format!(
                    "{name}: size mismatch (manifest {}, disk {actual_size})",
                    size.py_repr()
                ));
            }
        }
    }
    Ok(problems)
}

/// Verify `manifest.json` hashes and that the archived PCB matches `pcb`.
pub fn verify_bundle(manifest_path: &Path, pcb_path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let manifest = pyjson::loads(&text).map_err(|e| {
        crate::utils::pyjsondecode::json_decode_error(&text).unwrap_or_else(|| e.to_string())
    })?;
    if !matches!(manifest, Json::Obj(_)) {
        return Err(format!(
            "'{}' object has no attribute 'get'",
            manifest.py_type_name()
        ));
    }
    let files = match manifest.get("files") {
        Some(Json::Obj(f)) if f.iter().any(|(k, _)| k == "kicad_project.zip") => f.clone(),
        _ => return Err("missing hash-bound kicad_project.zip".into()),
    };
    let bundle = resolve(manifest_path.parent().unwrap_or(Path::new(".")));
    for (name, info) in &files {
        let path = Path::new(name);
        if path.is_absolute()
            || path.components().any(|c| matches!(c, Component::ParentDir))
            || name.contains('\\')
        {
            return Err(format!("unsafe manifest path: {name}"));
        }
        if !resolve(&bundle.join(path)).starts_with(&bundle) {
            return Err(format!("manifest path leaves bundle: {name}"));
        }
        if name == "manifest.json" {
            continue;
        }
        let digest = info.get("sha256").and_then(Json::as_str);
        if !digest.is_some_and(|d| SHA_RE.is_match(d)) {
            return Err(format!("missing or invalid SHA-256: {name}"));
        }
    }
    let archive_path = bundle.join("kicad_project.zip");
    if !archive_path.is_file() {
        return Err("missing kicad_project.zip".into());
    }
    let problems = verify_manifest(manifest_path)?;
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let pcb_name = pcb_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let file = std::fs::File::open(&archive_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.name().to_string()))
        .collect();
    let base = |n: &str| -> String {
        Path::new(n)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let matches: Vec<&String> = names.iter().filter(|n| base(n) == pcb_name).collect();
    if matches.len() != 1 || *matches[0] != pcb_name {
        return Err("project archive must contain one exact source PCB member".into());
    }
    let mut member = zip.by_name(&pcb_name).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    member.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let current = std::fs::read(pcb_path).map_err(|e| e.to_string())?;
    if sha256_hex(&buf) != sha256_hex(&current) {
        return Err("routed PCB content differs from the manifest-bound project archive".into());
    }
    Ok(())
}
