//! Full verification against immutable working/baseline snapshots.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};
#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    baseline: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long, value_enum, default_value = "error")]
    gate: Gate,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Gate {
    Error,
    Warning,
    Never,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stage {
    completed: bool,
    report: kct::check_report::Envelope,
}
#[derive(Serialize, Deserialize)]
struct Cache {
    valid_until: Option<u64>,
    stages: BTreeMap<String, Stage>,
}
fn selected_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some(
            "kicad_pcb"
                | "kicad_sch"
                | "kicad_pro"
                | "kicad_dru"
                | "kicad_mod"
                | "kicad_sym"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "csv"
                | "md"
        )
    )
}
fn copy_snapshot(root: &Path, dest: &Path) -> Result<String> {
    let mut paths = Vec::new();
    for e in walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        !matches!(
            e.file_name().to_str(),
            Some(
                ".git"
                    | "target"
                    | "node_modules"
                    | ".kct-region-cache"
                    | ".kicadmium"
                    | ".cache"
                    | ".portal"
                    | "dist"
            )
        )
    }) {
        let e = e?;
        if e.file_type().is_file() && selected_file(e.path()) {
            paths.push(e.path().to_path_buf())
        }
    }
    paths.sort();
    let mut digest = Sha256::new();
    for path in paths {
        let relative = path.strip_prefix(root)?;
        let bytes = std::fs::read(&path)?;
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(&bytes);
        digest.update([0]);
        let out = dest.join(relative);
        std::fs::create_dir_all(out.parent().context("snapshot parent")?)?;
        std::fs::write(out, bytes)?;
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn git_snapshot(root: &Path, revision: &str, dest: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])
        .output()?;
    if !output.status.success() {
        bail!("invalid baseline revision")
    }
    let commit = String::from_utf8(output.stdout)?.trim().to_owned();
    let prefix = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-prefix"])
        .output()?;
    if !prefix.status.success() {
        bail!("cannot resolve baseline project directory")
    }
    let prefix = String::from_utf8(prefix.stdout)?;
    let prefix = prefix.trim().trim_end_matches('/');
    let tree = if prefix.is_empty() {
        commit.clone()
    } else {
        format!("{commit}:{prefix}")
    };
    let archive = tempfile::NamedTempFile::new()?;
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["archive", "--format=tar", "--output"])
        .arg(archive.path())
        .arg(&tree)
        .output()?;
    if !result.status.success() {
        bail!(
            "archive baseline: {}",
            String::from_utf8_lossy(&result.stderr)
        )
    }
    let result = Command::new("tar")
        .arg("-xf")
        .arg(archive.path())
        .arg("-C")
        .arg(dest)
        .output()?;
    if !result.status.success() {
        bail!(
            "extract baseline: {}",
            String::from_utf8_lossy(&result.stderr)
        )
    }
    Ok(commit)
}
fn failed(tool: &str, error: impl std::fmt::Display) -> Stage {
    Stage {
        completed: false,
        report: kct::check_report::Envelope::new(
            tool,
            false,
            json!({"findings":[{"rule":"stage.failed","severity":"error","message":error.to_string()}]}),
        ),
    }
}
fn from_check(tool: &str, result: Result<shared::CheckResponse>) -> Stage {
    match result {
        Ok(r) => match serde_json::from_str::<Value>(&r.report) {
            Ok(v) => {
                if tool == "quality"
                    && v["kct"]["commands"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|c| c["parsed"] != true)
                {
                    return failed(tool, "a required quality subprocess produced no report");
                }
                let report = kct::check_report::Envelope::new(tool, r.ok, v);
                if !r.ok && report.findings.is_empty() {
                    return failed(tool, "check failed without reporting findings");
                }
                Stage {
                    completed: true,
                    report,
                }
            }
            Err(e) => failed(tool, format!("{}: {e}", r.message.unwrap_or(r.stderr))),
        },
        Err(e) => failed(tool, e),
    }
}
fn lint(project: &crate::ProjectContext) -> Result<(Stage, Stage, Option<u64>)> {
    let board = crate::pick_project_file(project, "kicad_pcb")?.context("no PCB")?;
    let policy = kct::lint::board_file::BoardLintFile::load_or_default(
        &kct::lint::board_file::BoardLintFile::path_for(&board),
        &project.id,
    )?;
    let source = std::fs::read_to_string(board)?;
    let checked = kct::lint::board_file::lint_board(&source, &policy, kct::lint::review::now())?;
    let expiry = policy
        .exceptions
        .iter()
        .filter_map(|e| e.expires_at)
        .filter(|t| *t > kct::lint::review::now())
        .min();
    let stale:Vec<_>=checked.audit.iter().filter(|a|a.status!=kct::lint::board_file::ExceptionStatus::Applied).map(|a|json!({"rule":format!("lint.exception.{}",a.status),"severity":"warning","key":a.key,"message":a.detail})).collect();
    Ok((
        Stage {
            completed: true,
            report: kct::check_report::Envelope::new(
                "lint",
                true,
                serde_json::to_value(checked.report)?,
            ),
        },
        Stage {
            completed: true,
            report: kct::check_report::Envelope::new(
                "stale",
                stale.is_empty(),
                json!({"findings":stale}),
            ),
        },
        expiry,
    ))
}
fn docs(project: &crate::ProjectContext) -> Result<Stage> {
    let manifest = project.repo_root.join(".kicad-pcb.json");
    let mut findings = Vec::new();
    if manifest.exists() {
        let v: Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
        if let Some(p) = v["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["id"] == project.id)
        {
            if p.get("derivedFiles").is_some() {
                let files: Vec<kct::derived_docs::DerivedFile> =
                    serde_json::from_value(p["derivedFiles"].clone())?;
                let board = crate::pick_project_file(project, "kicad_pcb")?.context("no PCB")?;
                let pcb = kct::schema::pcb::Pcb::load(board)?;
                for output in kct::derived_docs::plan(&project.root, &pcb, &files)? {
                    if output.stale {
                        findings.push(json!({"rule":"docs.stale","severity":"error","message":format!("{} needs docs sync",output.path.strip_prefix(&project.root)?.display())}));
                    }
                }
            }
        }
    }
    Ok(Stage {
        completed: true,
        report: kct::check_report::Envelope::new(
            "docs",
            findings.is_empty(),
            json!({"findings":findings}),
        ),
    })
}
async fn collect(root: &Path, project_id: &str, cache: &Path) -> Result<BTreeMap<String, Stage>> {
    if cache.exists() {
        let c: Cache = serde_json::from_slice(&std::fs::read(cache)?)?;
        if c.valid_until.is_none_or(|t| t > kct::lint::review::now()) {
            return Ok(c.stages);
        }
    }
    let project = crate::cli_project_context(root, Some(project_id))?;
    let mut stages = BTreeMap::new();
    // Sequential subprocess stages keep peak memory bounded; snapshots remain immutable.
    stages.insert(
        "drc".into(),
        from_check("drc", crate::run_check(&project, "drc").await),
    );
    stages.insert(
        "erc".into(),
        from_check("erc", crate::run_check(&project, "erc").await),
    );
    stages.insert(
        "quality".into(),
        from_check("quality", crate::quality::run_check(&project).await),
    );
    let mut valid_until = None;
    match lint(&project) {
        Ok((lint, stale, expiry)) => {
            stages.insert("lint".into(), lint);
            stages.insert("stale".into(), stale);
            valid_until = expiry;
        }
        Err(e) => {
            stages.insert("lint".into(), failed("lint", &e));
            stages.insert("stale".into(), failed("stale", e));
        }
    }
    stages.insert(
        "docs".into(),
        docs(&project).unwrap_or_else(|e| failed("docs", e)),
    );
    if stages.values().all(|s| s.completed) {
        std::fs::create_dir_all(cache.parent().context("cache parent")?)?;
        kct::fsutil::atomic_write(
            cache,
            &serde_json::to_vec(&Cache {
                valid_until,
                stages: stages.clone(),
            })?,
        )?;
    }
    Ok(stages)
}
fn identity(f: &Value) -> String {
    if let Some(key) = f["key"].as_str() {
        return format!("{}:{}:{}", key, f["severity"], f["state"]);
    }
    // Native reports have no persistent keys. Preserve item geometry/UUIDs while
    // dropping producer timestamps and unstable snapshot file names.
    let value = json!({"rule":f["rule"],"severity":f["severity"],"nets":f["nets"],"refs":f["refs"],"at":f["at"],"message":f["message"],"items":f["items"]});
    kct::lint::hash(&value.to_string())
}
fn delta(before: &Stage, after: &Stage) -> Value {
    let map = |s: &Stage| {
        s.report
            .findings
            .iter()
            .map(|f| (identity(f), f.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let a = map(before);
    let b = map(after);
    let added: Vec<_> = b
        .iter()
        .filter(|(k, _)| !a.contains_key(*k))
        .map(|(_, f)| f.clone())
        .collect();
    let removed: Vec<_> = a
        .iter()
        .filter(|(k, _)| !b.contains_key(*k))
        .map(|(_, f)| f.clone())
        .collect();
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    for (kind, rows) in [("added", &added), ("removed", &removed)] {
        for f in rows {
            let key = format!("{} nets={}", f["rule"], f["nets"]);
            let g = groups.entry(key).or_insert_with(
                || json!({"rule":f["rule"],"nets":f["nets"],"added":0,"removed":0}),
            );
            g[kind] = json!(g[kind].as_u64().unwrap() + 1);
        }
    }
    json!({"completed":after.completed,"before":a.len(),"after":b.len(),"added":added,"removed":removed,"groups":groups.into_values().collect::<Vec<_>>()})
}
pub(crate) async fn run(args: Args) -> Result<i32> {
    let cwd = args.cwd.canonicalize()?;
    let project = crate::cli_project_context(&cwd, args.project.as_deref())?;
    let working = tempfile::tempdir()?;
    let source_hash = copy_snapshot(&project.repo_root, working.path())?;
    let executable = std::fs::read(std::env::current_exe()?)?;
    let tool = format!(
        "{:x}:{:?}",
        Sha256::digest(executable),
        crate::tool_version(crate::kicad_cli()).await
    );
    let cache = crate::cache_dir_for(&project.root)?.join("verify");
    let key = |s: &str| kct::lint::hash(&format!("verify/v1:{}:{tool}:{s}", project.id));
    let current = collect(
        working.path(),
        &project.id,
        &cache.join(format!("{}.json", key(&source_hash))),
    )
    .await?;
    let before = if let Some(baseline) = &args.baseline {
        let snapshot = tempfile::tempdir()?;
        let identity = if Path::new(baseline).is_file() {
            copy_snapshot(&project.repo_root, snapshot.path())?;
            let board = crate::pick_project_file(&project, "kicad_pcb")?.context("no PCB")?;
            let bytes = std::fs::read(baseline)?;
            std::fs::write(
                snapshot
                    .path()
                    .join(board.strip_prefix(&project.repo_root)?),
                &bytes,
            )?;
            format!("file:{source_hash}:{:x}", Sha256::digest(bytes))
        } else {
            git_snapshot(&project.repo_root, baseline, snapshot.path())?
        };
        collect(
            snapshot.path(),
            &project.id,
            &cache.join(format!("{}.json", key(&identity))),
        )
        .await?
    } else {
        BTreeMap::new()
    };
    let empty = Stage {
        completed: true,
        report: kct::check_report::Envelope::new("empty", true, json!({"findings":[]})),
    };
    let mut stages = BTreeMap::new();
    let mut failed = false;
    for (name, after) in &current {
        let prior = before.get(name).unwrap_or(&empty);
        let diff = delta(prior, after);
        failed |= !after.completed || !prior.completed;
        failed |= diff["added"].as_array().unwrap().iter().any(|f| {
            f["state"] != "ignored"
                && match args.gate {
                    Gate::Never => false,
                    Gate::Error => f["severity"] == "error",
                    Gate::Warning => f["severity"] == "error" || f["severity"] == "warning",
                }
        });
        stages.insert(name.clone(), diff);
    }
    let final_snapshot = tempfile::tempdir()?;
    let source_unchanged = copy_snapshot(&project.repo_root, final_snapshot.path())? == source_hash;
    failed |= !source_unchanged;
    let report = json!({"schema":"kicadmium.verify/v1","ok":!failed,"baseline":args.baseline,"source_sha256":source_hash,"source_unchanged":source_unchanged,"stages":stages,"full_board_gate":true});
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?)
    } else {
        for (name, s) in &stages {
            println!(
                "{name}: {} → {} (+{}, −{}){}",
                s["before"],
                s["after"],
                s["added"].as_array().unwrap().len(),
                s["removed"].as_array().unwrap().len(),
                if s["completed"] == true {
                    ""
                } else {
                    " FAILED"
                }
            );
            for g in s["groups"].as_array().unwrap().iter().take(5) {
                println!(
                    "  {} nets={}: +{} −{}",
                    g["rule"], g["nets"], g["added"], g["removed"]
                );
            }
        }
        if !source_unchanged {
            println!("Source changed during verification; rerun before committing");
        }
        println!(
            "{}",
            if failed {
                "Verification gate failed"
            } else {
                "Verification gate passed"
            }
        );
    }
    Ok(i32::from(failed))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diff_groups_added_and_removed_without_count_only_matching() {
        let s = |f| Stage {
            completed: true,
            report: kct::check_report::Envelope::new("lint", true, json!({"findings":f})),
        };
        let a = s(json!([{"key":"old","rule":"clearance","severity":"error","nets":["N"]}]));
        let b = s(json!([{"key":"new","rule":"clearance","severity":"error","nets":["N"]}]));
        let d = delta(&a, &b);
        assert_eq!(d["added"].as_array().unwrap().len(), 1);
        assert_eq!(d["removed"].as_array().unwrap().len(), 1);
        assert_eq!(d["groups"][0]["added"], 1);
        assert!(delta(&a, &a)["added"].as_array().unwrap().is_empty());
    }
    #[test]
    fn snapshots_do_not_copy_build_artifacts_or_change_sources() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("target")).unwrap();
        std::fs::write(d.path().join("target/ignored.json"), "{}").unwrap();
        std::fs::write(d.path().join("board.kicad_pcb"), "original").unwrap();
        let out = tempfile::tempdir().unwrap();
        let a = copy_snapshot(d.path(), out.path()).unwrap();
        assert!(!out.path().join("target").exists());
        std::fs::write(out.path().join("board.kicad_pcb"), "changed").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("board.kicad_pcb")).unwrap(),
            "original"
        );
        let other = tempfile::tempdir().unwrap();
        assert_eq!(a, copy_snapshot(d.path(), other.path()).unwrap());
    }
}
