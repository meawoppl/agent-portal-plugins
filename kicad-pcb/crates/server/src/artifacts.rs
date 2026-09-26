//! Stage executors, JLCPCB placement offsets, publish manifests, and
//! fab-output staleness.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    Json,
};
use kicad_pcb_shared::{PublishStatus, SourceHashes};
use serde::{Deserialize, Serialize};

use crate::{
    jobs::{self, Outcome, Stage},
    revision, AppError, AppState, ProjectContext, ProjectQuery,
};

/// Publish manifest file name, written in the project's fab dir.
pub(crate) const MANIFEST_NAME: &str = ".kicad-pcb-build.json";

/// Build status strip (served at /kicad-pcb/build-strip.js).
pub(crate) const BUILD_STRIP_JS: &str = include_str!("build_strip.js");

// ---------------------------------------------------------------------------
// Artifact directories

fn artifact_rel(project: &ProjectContext, kind: &str) -> PathBuf {
    let artifacts = project.config.artifacts.as_ref();
    let fab = artifacts
        .and_then(|item| item.fab.clone())
        .unwrap_or_else(|| PathBuf::from("fab"));
    let configured = match kind {
        "fab" => return fab,
        "gerbers" => artifacts.and_then(|item| item.gerbers.clone()),
        "jlcpcb" => artifacts.and_then(|item| item.jlcpcb.clone()),
        "checks" => artifacts.and_then(|item| item.checks.clone()),
        _ => None,
    };
    configured.unwrap_or_else(|| fab.join(kind))
}

pub(crate) fn artifact_dir(project: &ProjectContext, kind: &str) -> PathBuf {
    project.root.join(artifact_rel(project, kind))
}

fn is_plain_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

/// Artifact paths are written by publish, so they must stay inside the
/// project root.
pub(crate) fn validate_artifact_config(project: &crate::ProjectConfig) -> Result<()> {
    let Some(artifacts) = &project.artifacts else {
        return Ok(());
    };
    for (label, path) in [
        ("fab", &artifacts.fab),
        ("checks", &artifacts.checks),
        ("gerbers", &artifacts.gerbers),
        ("jlcpcb", &artifacts.jlcpcb),
        ("docs", &artifacts.docs),
    ] {
        if let Some(path) = path {
            if !is_plain_relative(path) {
                return Err(anyhow!(
                    "project {:?} artifacts.{label} must be a relative path inside the project root",
                    project.id
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn auto_publish(project: &ProjectContext) -> bool {
    project
        .config
        .artifacts
        .as_ref()
        .and_then(|item| item.auto_publish)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Stage execution

fn board(project: &ProjectContext) -> Result<PathBuf> {
    crate::pick_project_file(project, "kicad_pcb")?
        .ok_or_else(|| anyhow!("Select a single .kicad_pcb or set kicad.pcb in .kicad-pcb.json"))
}

fn schematic(project: &ProjectContext) -> Result<PathBuf> {
    crate::pick_project_file(project, "kicad_sch")?.ok_or_else(|| {
        anyhow!("Select a single .kicad_sch or set kicad.schematic in .kicad-pcb.json")
    })
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn json_outcome(value: &serde_json::Value) -> Outcome {
    Outcome {
        ok: value["ok"].as_bool().unwrap_or(false),
        message: value["message"].as_str().map(str::to_string),
        log: format!(
            "{}{}",
            value["stdout"].as_str().unwrap_or_default(),
            value["stderr"].as_str().unwrap_or_default()
        ),
    }
}

async fn kicad(project: &ProjectContext, args: Vec<OsString>) -> Result<Outcome> {
    let cli = crate::kicad_cli().ok_or_else(|| anyhow!("kicad-cli is required"))?;
    let output = crate::run_command(&cli, &args, &project.root).await?;
    Ok(Outcome {
        ok: output.status == 0,
        message: (output.status != 0).then(|| format!("kicad-cli exited {}", output.status)),
        log: format!("{}{}", output.stdout, output.stderr),
    })
}

fn violation_count(kind: &str, report: &str) -> Option<usize> {
    let value: serde_json::Value = serde_json::from_str(report).ok()?;
    let len = |key: &str| value[key].as_array().map_or(0, Vec::len);
    Some(if kind == "erc" {
        value["sheets"]
            .as_array()?
            .iter()
            .map(|sheet| sheet["violations"].as_array().map_or(0, Vec::len))
            .sum()
    } else {
        len("violations") + len("unconnected_items") + len("schematic_parity")
    })
}

/// Runs one build stage, writing outputs into `out`.
pub(crate) async fn execute(stage: Stage, project: &ProjectContext, out: &Path) -> Result<Outcome> {
    match stage {
        Stage::Erc | Stage::Drc => {
            let kind = stage.id();
            let check = crate::run_check(project, kind).await?;
            if !check.report.is_empty() {
                tokio::fs::write(out.join(format!("{kind}.json")), &check.report).await?;
            }
            tokio::fs::write(out.join("check.json"), serde_json::to_vec_pretty(&check)?).await?;
            let message = if check.ok {
                None
            } else if let Some(count) = violation_count(kind, &check.report) {
                Some(format!(
                    "{count} violation{}",
                    if count == 1 { "" } else { "s" }
                ))
            } else {
                check
                    .message
                    .clone()
                    .or_else(|| Some("check failed".into()))
            };
            Ok(Outcome {
                ok: check.ok,
                message,
                log: format!("{}{}", check.stdout, check.stderr),
            })
        }
        Stage::Bom => {
            let sch = schematic(project)?;
            let target = out.join(format!("{}-bom.csv", stem(&sch)));
            kicad(
                project,
                vec![
                    "sch".into(),
                    "export".into(),
                    "bom".into(),
                    "--output".into(),
                    target.into(),
                    sch.into(),
                ],
            )
            .await
        }
        Stage::SchPdf => {
            let sch = schematic(project)?;
            let target = out.join(format!("{}-schematic.pdf", stem(&sch)));
            kicad(
                project,
                vec![
                    "sch".into(),
                    "export".into(),
                    "pdf".into(),
                    "--output".into(),
                    target.into(),
                    sch.into(),
                ],
            )
            .await
        }
        Stage::Gerbers => Ok(json_outcome(&crate::export_fab(project, out, false).await?)),
        Stage::Jlcpcb => Ok(json_outcome(&crate::export_fab(project, out, true).await?)),
        Stage::Glb => {
            let target = out.join(format!("{}.glb", stem(&board(project)?)));
            Ok(json_outcome(&crate::export_glb(project, &target).await?))
        }
        Stage::Step => {
            let pcb = board(project)?;
            let target = out.join(format!("{}.step", stem(&pcb)));
            kicad(
                project,
                vec![
                    "pcb".into(),
                    "export".into(),
                    "step".into(),
                    "--force".into(),
                    "--subst-models".into(),
                    "--output".into(),
                    target.into(),
                    pcb.into(),
                ],
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// JLCPCB placement offsets

/// Offsets file: `manufacturer.placementOffsets` (project-relative) or
/// `docs/jlcpcb-placement-offsets.json` when present.
pub(crate) fn placement_offsets_path(project: &ProjectContext) -> Option<PathBuf> {
    let configured = project
        .config
        .manufacturer
        .as_ref()
        .and_then(|value| value.get("placementOffsets"))
        .and_then(|value| value.as_str())
        .map(PathBuf::from);
    let rel = configured.unwrap_or_else(|| PathBuf::from("docs/jlcpcb-placement-offsets.json"));
    if !is_plain_relative(&rel) {
        return None;
    }
    let path = project.root.join(rel);
    path.is_file().then_some(path)
}

#[derive(Debug, Default, Deserialize)]
struct PlacementOffset {
    #[serde(default)]
    rotation_offset_degrees: f64,
    #[serde(default)]
    cpl_offset_x_mm: f64,
    #[serde(default)]
    cpl_offset_y_mm: f64,
    verified_native_rotation_degrees: Option<f64>,
}

/// Applies per-LCSC-part rotation/translation corrections to a JLCPCB CPL,
/// with the same safety checks as the reference carrier export: translated
/// parts must be top-side and still at their verified native rotation, and
/// rotation corrections are top-side only.
pub(crate) fn apply_placement_offsets(
    project: &ProjectContext,
    bom: &Path,
    cpl: &Path,
) -> Result<Option<usize>> {
    let Some(offsets_path) = placement_offsets_path(project) else {
        return Ok(None);
    };
    let offsets: BTreeMap<String, serde_json::Value> = serde_json::from_str(
        &std::fs::read_to_string(&offsets_path)
            .with_context(|| format!("read {}", offsets_path.display()))?,
    )
    .with_context(|| format!("parse {}", offsets_path.display()))?;
    let mut part_by_ref = BTreeMap::new();
    let mut reader = csv::Reader::from_path(bom)?;
    let headers = reader.headers()?.clone();
    let col = |name: &str| headers.iter().position(|value| value == name);
    let (Some(designator), Some(lcsc)) = (col("Designator"), col("LCSC Part #")) else {
        return Err(anyhow!("JLCPCB BOM lacks Designator / LCSC Part # columns"));
    };
    for row in reader.records() {
        let row = row?;
        let part = row.get(lcsc).unwrap_or_default().trim().to_string();
        for reference in row.get(designator).unwrap_or_default().split(',') {
            part_by_ref.insert(reference.trim().to_string(), part.clone());
        }
    }

    let mut reader = csv::Reader::from_path(cpl)?;
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["Designator", "Mid X", "Mid Y", "Layer", "Rotation"])?;
    let mut adjusted = 0;
    for row in reader.records() {
        let row = row?;
        let reference = row.get(0).unwrap_or_default();
        let parse = |index: usize| -> Result<f64> {
            row.get(index)
                .unwrap_or_default()
                .trim()
                .parse::<f64>()
                .with_context(|| format!("CPL {reference}: bad number"))
        };
        let (mut x, mut y, layer, native) =
            (parse(1)?, parse(2)?, row.get(3).unwrap_or("T"), parse(4)?);
        let mut rotation = native;
        let part = part_by_ref.get(reference).cloned().unwrap_or_default();
        if let Some(raw) = offsets.get(&part) {
            let offset: PlacementOffset = serde_json::from_value(raw.clone())
                .with_context(|| format!("placement offset for {part}"))?;
            if offset.cpl_offset_x_mm != 0.0 || offset.cpl_offset_y_mm != 0.0 {
                let verified = offset.verified_native_rotation_degrees.ok_or_else(|| {
                    anyhow!("placement offset for {part} translates but lacks verified_native_rotation_degrees")
                })?;
                if layer != "T" || native.rem_euclid(360.0) != verified.rem_euclid(360.0) {
                    return Err(anyhow!(
                        "{reference} ({part}): placement changed since offset was verified (side {layer}, rotation {native}); reverify the translation"
                    ));
                }
                x += offset.cpl_offset_x_mm;
                y += offset.cpl_offset_y_mm;
            }
            if offset.rotation_offset_degrees != 0.0 {
                if layer != "T" {
                    return Err(anyhow!(
                        "{reference} ({part}): bottom-side rotation offsets need separate verification"
                    ));
                }
                rotation += offset.rotation_offset_degrees;
            }
            adjusted += 1;
        }
        writer.write_record([
            reference.to_string(),
            format!("{x:.6}"),
            format!("{y:.6}"),
            layer.to_string(),
            format!("{:.6}", rotation.rem_euclid(360.0)),
        ])?;
    }
    std::fs::write(cpl, writer.into_inner()?)?;
    Ok(Some(adjusted))
}

// ---------------------------------------------------------------------------
// Publish

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublishedFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublishedStage {
    pub input_key: String,
    pub ok: bool,
    pub message: Option<String>,
    pub files: Vec<PublishedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublishManifest {
    pub version: u32,
    pub generator: String,
    pub project: String,
    pub published_at_ms: u64,
    pub kicad_version: String,
    pub hashes: SourceHashes,
    pub source_files: BTreeMap<String, String>,
    pub stages: BTreeMap<String, PublishedStage>,
}

fn manifest_path(project: &ProjectContext) -> PathBuf {
    artifact_dir(project, "fab").join(MANIFEST_NAME)
}

pub(crate) fn read_manifest(project: &ProjectContext) -> Option<PublishManifest> {
    serde_json::from_str(&std::fs::read_to_string(manifest_path(project)).ok()?).ok()
}

/// Cache dirs referenced by the published manifest (kept by GC).
pub(crate) fn published_stage_dirs(project: &ProjectContext) -> HashSet<String> {
    read_manifest(project)
        .map(|manifest| {
            manifest
                .stages
                .iter()
                .filter_map(|(id, stage)| {
                    Stage::from_id(id).map(|item| jobs::stage_dir_name(item, &stage.input_key))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Destination dir and file filter for each stage.
fn publish_target(project: &ProjectContext, stage: Stage, file: &str) -> Option<PathBuf> {
    let dir = match stage {
        Stage::Erc | Stage::Drc => {
            if file != format!("{}.json", stage.id()) {
                return None;
            }
            artifact_dir(project, "checks")
        }
        Stage::Bom => artifact_dir(project, "bom"),
        Stage::Gerbers => artifact_dir(project, "gerbers"),
        Stage::Jlcpcb => artifact_dir(project, "jlcpcb"),
        Stage::SchPdf | Stage::Glb | Stage::Step => artifact_dir(project, "fab"),
    };
    Some(dir.join(file))
}

pub(crate) fn publish(
    project: &ProjectContext,
    hashes: &SourceHashes,
    source_files: &BTreeMap<String, String>,
    stages: &jobs::BuildSnapshot,
    kicad_version: &str,
) -> Result<serde_json::Value> {
    let missing: Vec<&str> = stages
        .iter()
        .filter(|(_, (_, result, _))| result.is_none())
        .map(|(stage, _)| stage.id())
        .collect();
    if !missing.is_empty() {
        return Err(anyhow!(
            "stages without a finished build: {}",
            missing.join(", ")
        ));
    }
    let previous = read_manifest(project);
    let mut manifest = PublishManifest {
        version: 1,
        generator: "kicad-pcb".into(),
        project: project.id.clone(),
        published_at_ms: crate::now_ms(),
        kicad_version: kicad_version.to_string(),
        hashes: hashes.clone(),
        source_files: source_files.clone(),
        stages: BTreeMap::new(),
    };
    let mut written = HashSet::new();
    for (stage, (key, result, dir)) in stages {
        let result = result.as_ref().expect("checked above");
        let mut files = Vec::new();
        for name in &result.outputs {
            let Some(target) = publish_target(project, *stage, name) else {
                continue;
            };
            let source = dir.join(name);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = target.with_file_name(format!(".{name}.kicad-pcb-tmp"));
            std::fs::copy(&source, &tmp).with_context(|| format!("copy {}", source.display()))?;
            std::fs::rename(&tmp, &target)?;
            let rel = crate::rel(&project.root, &target)?;
            written.insert(rel.clone());
            files.push(PublishedFile {
                path: rel,
                sha256: revision::file_sha256(&target).unwrap_or_default(),
                size: std::fs::metadata(&target)?.len(),
            });
        }
        manifest.stages.insert(
            stage.id().to_string(),
            PublishedStage {
                input_key: key.clone(),
                ok: result.ok,
                message: result.message.clone(),
                files,
            },
        );
    }
    // Remove files this plugin published previously that the new build no
    // longer produces (never touches files it did not write).
    let mut removed = Vec::new();
    if let Some(previous) = previous {
        for stage in previous.stages.values() {
            for file in &stage.files {
                if written.contains(&file.path) || !is_plain_relative(Path::new(&file.path)) {
                    continue;
                }
                let path = project.root.join(&file.path);
                if path.is_file() && std::fs::remove_file(&path).is_ok() {
                    removed.push(file.path.clone());
                }
            }
        }
    }
    let path = manifest_path(project);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!("{MANIFEST_NAME}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&manifest)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(serde_json::json!({
        "ok": true,
        "project": project.id,
        "manifest": crate::rel(&project.root, &path)?,
        "revision": hashes.revision,
        "files": written.len(),
        "removed": removed,
    }))
}

// ---------------------------------------------------------------------------
// Staleness

/// Compares published fab outputs with the current source hashes and stage
/// keys. Falls back to legacy `revision-sha256.json` files.
pub(crate) fn staleness(
    project: &ProjectContext,
    source_files: &BTreeMap<String, String>,
    keys: &BTreeMap<Stage, String>,
) -> PublishStatus {
    let mut status = PublishStatus::default();
    if let Some(manifest) = read_manifest(project) {
        status.published = true;
        status.manifest = crate::rel(&project.root, &manifest_path(project)).ok();
        status.published_at_ms = Some(manifest.published_at_ms);
        status.published_revision = Some(manifest.hashes.revision.clone());
        for (id, published) in &manifest.stages {
            let Some(stage) = Stage::from_id(id) else {
                continue;
            };
            if let Some(current) = keys.get(&stage) {
                if current != &published.input_key {
                    status.stale_stages.push(id.clone());
                }
            }
            for file in &published.files {
                let path = project.root.join(&file.path);
                match revision::file_sha256(&path) {
                    Some(sha) if sha == file.sha256 => {}
                    Some(_) => status
                        .reasons
                        .push(format!("{} modified since publish", file.path)),
                    None => status
                        .reasons
                        .push(format!("{} missing since publish", file.path)),
                }
            }
        }
        let changed = changed_sources(&manifest.source_files, source_files);
        if !changed.is_empty() {
            status.reasons.insert(
                0,
                format!("sources changed since publish: {}", summarize(&changed)),
            );
        }
        if !status.stale_stages.is_empty() && changed.is_empty() {
            status
                .reasons
                .insert(0, "build inputs changed (KiCad version or config)".into());
        }
        status.stale = !status.stale_stages.is_empty() || !status.reasons.is_empty();
        return status;
    }

    // Legacy: <checks>/revision-sha256.json maps project paths to sha256.
    for candidate in [
        artifact_dir(project, "checks").join("revision-sha256.json"),
        artifact_dir(project, "fab").join("revision-sha256.json"),
    ] {
        let Ok(text) = std::fs::read_to_string(&candidate) else {
            continue;
        };
        let Ok(recorded) = serde_json::from_str::<BTreeMap<String, String>>(&text) else {
            continue;
        };
        status.manifest = crate::rel(&project.root, &candidate).ok();
        status.published = true;
        let mut sch = false;
        let mut pcb = false;
        for (path, sha) in &recorded {
            // Recorded fab outputs are skipped; only sources are compared.
            let Some(current) = source_files.get(path) else {
                continue;
            };
            if current != sha {
                status.reasons.push(format!(
                    "{} changed since {}",
                    path,
                    status.manifest.clone().unwrap_or_default()
                ));
                match revision::source_kind(Path::new(path)) {
                    Some(revision::SourceKind::Sch) => sch = true,
                    Some(revision::SourceKind::Pcb) => pcb = true,
                    _ => {
                        sch = true;
                        pcb = true;
                    }
                }
            }
        }
        for stage in keys.keys() {
            let (needs_sch, needs_pcb) = stage.deps();
            if (needs_sch && sch) || (needs_pcb && pcb) {
                status.stale_stages.push(stage.id().to_string());
            }
        }
        status.stale = !status.reasons.is_empty();
        break;
    }
    status
}

fn changed_sources(
    recorded: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed: Vec<String> = current
        .iter()
        .filter(|(path, sha)| recorded.get(*path) != Some(*sha))
        .map(|(path, _)| path.clone())
        .chain(
            recorded
                .keys()
                .filter(|path| !current.contains_key(*path))
                .cloned(),
        )
        .collect();
    changed.sort();
    changed.dedup();
    changed
}

fn summarize(items: &[String]) -> String {
    let mut text = items.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 4 {
        text.push_str(&format!(" (+{} more)", items.len() - 4));
    }
    text
}

/// Build/publish status for `doctor --json`.
pub(crate) fn doctor_report(
    projects: &[ProjectContext],
    kicad_version: Option<&str>,
    config: Option<&jobs::BuildConfig>,
) -> serde_json::Value {
    let settings = jobs::BuildSettings::from_config(config);
    let kicad_version = kicad_version.unwrap_or("unavailable");
    let entries = projects
        .iter()
        .map(|project| {
            let hashes = match revision::project_hashes(project) {
                Ok(hashes) => hashes,
                Err(err) => {
                    return serde_json::json!({"project": project.id, "error": err.to_string()})
                }
            };
            let keys: BTreeMap<Stage, String> = settings
                .stages
                .iter()
                .map(|stage| {
                    (
                        *stage,
                        jobs::stage_key(*stage, project, &hashes.hashes, kicad_version),
                    )
                })
                .collect();
            let cached = jobs::builds_dir(project)
                .map(|dir| {
                    keys.iter()
                        .filter(|(stage, key)| {
                            jobs::read_result(&dir.join(jobs::stage_dir_name(**stage, key)))
                                .is_some()
                        })
                        .map(|(stage, _)| stage.id())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            serde_json::json!({
                "project": project.id,
                "hashes": hashes.hashes,
                "source_files": hashes.files.len(),
                "cached_stages": cached,
                "auto_publish": auto_publish(project),
                "publish": staleness(project, &hashes.files, &keys),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "auto": settings.auto,
        "concurrency": settings.concurrency,
        "debounce_ms": settings.debounce.as_millis() as u64,
        "keep_revisions": settings.keep_revisions,
        "stages": settings.stages.iter().map(|stage| stage.id()).collect::<Vec<_>>(),
        "projects": entries,
    })
}

// ---------------------------------------------------------------------------
// Gerber sources for the Gerber tab

#[derive(Debug, Serialize)]
struct GerberSource {
    path: String,
    url: String,
}

pub(crate) async fn gerbers_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let project = crate::selected_project(&state, query.project.as_deref())?;
    let status = state.builds.current_status(&project).await?;
    let encoded = crate::percent_encode(&project.id);
    for stage in [Stage::Gerbers, Stage::Jlcpcb] {
        let Some((result, _)) = state.builds.current_result(&project, stage) else {
            continue;
        };
        let files: Vec<GerberSource> = result
            .outputs
            .iter()
            .filter(|name| crate::is_embeddable_gerber_file(name))
            .map(|name| GerberSource {
                path: name.clone(),
                url: format!(
                    "/api/build/artifact?project={encoded}&stage={}&file={}",
                    stage.id(),
                    crate::percent_encode(name)
                ),
            })
            .collect();
        if files.is_empty() {
            continue;
        }
        return Ok(Json(serde_json::json!({
            "origin": "build",
            "stage": stage.id(),
            "revision": status.hashes.revision,
            "label": format!("Current build · rev {}", &status.hashes.revision[..8.min(status.hashes.revision.len())]),
            "files": files,
        })));
    }
    let root = project.root.clone();
    let files = tokio::task::spawn_blocking(move || crate::detect_files(&root)).await??;
    let files: Vec<GerberSource> = crate::selected_gerber_files(&files)
        .into_iter()
        .map(|path| GerberSource {
            url: format!(
                "/api/kicad/file?project={encoded}&path={}",
                crate::percent_encode(&path)
            ),
            path,
        })
        .collect();
    let stale = status.publish.stale;
    Ok(Json(serde_json::json!({
        "origin": if files.is_empty() { "none" } else { "published" },
        "revision": status.publish.published_revision,
        "label": if files.is_empty() {
            "No Gerbers yet: the build has not produced them".to_string()
        } else if stale {
            "Published files (stale vs sources) · build not ready".to_string()
        } else {
            "Published files · build not ready".to_string()
        },
        "files": files,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::revision::tests_support::project;
    use tempfile::TempDir;

    #[test]
    fn placement_offsets_apply_with_safety_checks() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        let project = project(&repo, "a", ".");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(
            repo.join("docs/jlcpcb-placement-offsets.json"),
            r#"{"C1":{"rotation_offset_degrees":-90},"C2":{"cpl_offset_y_mm":2.5,"verified_native_rotation_degrees":0}}"#,
        )
        .unwrap();
        let bom = repo.join("BOM.csv");
        let cpl = repo.join("CPL.csv");
        std::fs::write(
            &bom,
            "Comment,Designator,Footprint,LCSC Part #\nx,\"U1,U2\",f,C1\ny,J1,f,C2\nz,R1,f,C3\n",
        )
        .unwrap();
        std::fs::write(
            &cpl,
            "Designator,Mid X,Mid Y,Layer,Rotation\nU1,1,2,T,0\nU2,1,2,T,90\nJ1,5,5,T,0\nR1,3,3,B,180\n",
        )
        .unwrap();
        assert_eq!(
            apply_placement_offsets(&project, &bom, &cpl).unwrap(),
            Some(3)
        );
        let text = std::fs::read_to_string(&cpl).unwrap();
        assert!(text.contains("U1,1.000000,2.000000,T,270.000000"));
        assert!(text.contains("U2,1.000000,2.000000,T,0.000000"));
        assert!(text.contains("J1,5.000000,7.500000,T,0.000000"));
        assert!(text.contains("R1,3.000000,3.000000,B,180.000000"));

        std::fs::write(&cpl, "Designator,Mid X,Mid Y,Layer,Rotation\nJ1,5,5,T,90\n").unwrap();
        assert!(apply_placement_offsets(&project, &bom, &cpl).is_err());
    }

    #[test]
    fn staleness_uses_manifest_and_legacy_revision_files() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        let project = project(&repo, "a", ".");
        let keys = BTreeMap::from([
            (Stage::Gerbers, "k2".to_string()),
            (Stage::Erc, "e1".to_string()),
        ]);
        let sources = BTreeMap::from([("a.kicad_pcb".to_string(), "new".to_string())]);

        std::fs::create_dir_all(repo.join("fab/checks")).unwrap();
        std::fs::write(
            repo.join("fab/checks/revision-sha256.json"),
            r#"{"a.kicad_pcb":"old","fab/a.zip":"zzz"}"#,
        )
        .unwrap();
        let legacy = staleness(&project, &sources, &keys);
        assert!(legacy.stale);
        assert_eq!(legacy.stale_stages, vec!["gerbers".to_string()]);

        let manifest = PublishManifest {
            version: 1,
            generator: "kicad-pcb".into(),
            project: "a".into(),
            published_at_ms: 1,
            kicad_version: "10".into(),
            hashes: SourceHashes::default(),
            source_files: sources.clone(),
            stages: BTreeMap::from([
                (
                    "gerbers".to_string(),
                    PublishedStage {
                        input_key: "k1".into(),
                        ok: true,
                        message: None,
                        files: vec![],
                    },
                ),
                (
                    "erc".to_string(),
                    PublishedStage {
                        input_key: "e1".into(),
                        ok: true,
                        message: None,
                        files: vec![],
                    },
                ),
            ]),
        };
        std::fs::write(
            repo.join("fab").join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let current = staleness(&project, &sources, &keys);
        assert!(current.stale);
        assert_eq!(current.stale_stages, vec!["gerbers".to_string()]);
        assert!(
            published_stage_dirs(&project).contains(&jobs::stage_dir_name(Stage::Gerbers, "k1"))
        );
    }
}
