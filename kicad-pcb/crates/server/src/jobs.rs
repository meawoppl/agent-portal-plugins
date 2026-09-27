//! Automatic build pipeline: stage graph, content-addressed stage cache,
//! bounded job queue with supersede/cancel, and build status events.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use kicad_pcb_shared::{BuildStatus, PublishStatus, ServerEvent, SourceHashes, StageStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    process::Command,
    sync::{broadcast, watch, Notify, OwnedSemaphorePermit, Semaphore},
    time::Duration,
};

use crate::{
    artifacts,
    revision::{self, ProjectHashes},
    AppError, AppState, CommandOutput, ProjectContext,
};

// ---------------------------------------------------------------------------
// Stages

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Stage {
    Erc,
    Drc,
    Bom,
    SchPdf,
    Gerbers,
    Jlcpcb,
    Glb,
    Step,
    Quality,
}

impl Stage {
    pub(crate) const ALL: [Stage; 9] = [
        Stage::Erc,
        Stage::Drc,
        Stage::Bom,
        Stage::SchPdf,
        Stage::Gerbers,
        Stage::Jlcpcb,
        Stage::Glb,
        Stage::Step,
        Stage::Quality,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Stage::Erc => "erc",
            Stage::Drc => "drc",
            Stage::Bom => "bom",
            Stage::SchPdf => "schematic-pdf",
            Stage::Gerbers => "gerbers",
            Stage::Jlcpcb => "jlcpcb",
            Stage::Glb => "glb",
            Stage::Step => "step",
            Stage::Quality => "quality",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Stage::Erc => "ERC",
            Stage::Drc => "DRC",
            Stage::Bom => "BOM",
            Stage::SchPdf => "Sch PDF",
            Stage::Gerbers => "Gerbers",
            Stage::Jlcpcb => "JLCPCB",
            Stage::Glb => "GLB",
            Stage::Step => "STEP",
            Stage::Quality => "Quality",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|stage| stage.id() == id)
    }

    /// (depends on schematic set, depends on PCB set)
    pub(crate) fn deps(self) -> (bool, bool) {
        match self {
            Stage::Erc | Stage::Bom | Stage::SchPdf => (true, false),
            Stage::Drc | Stage::Jlcpcb | Stage::Quality => (true, true),
            Stage::Gerbers | Stage::Glb | Stage::Step => (false, true),
        }
    }

    /// Higher runs first. GLB feeds the live 3D view, checks feed the Checks
    /// tab, STEP is slow and least urgent.
    fn priority(self) -> u8 {
        match self {
            Stage::Glb => 90,
            Stage::Erc => 80,
            Stage::Drc => 70,
            Stage::Gerbers => 60,
            Stage::Bom => 50,
            Stage::Jlcpcb => 40,
            Stage::SchPdf => 30,
            Stage::Quality => 20,
            Stage::Step => 10,
        }
    }

    /// Stages that shell out to kicad-cli. Quality runs kct and in-plugin
    /// audits, so it works without KiCad.
    pub(crate) fn needs_kicad(self) -> bool {
        self != Stage::Quality
    }

    fn default_timeout(self) -> Duration {
        match self {
            Stage::Step => Duration::from_secs(900),
            _ => Duration::from_secs(300),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration

/// `build` section of `.kicad-pcb.json`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BuildConfig {
    /// Build warmed projects automatically when sources change (default true).
    pub auto: Option<bool>,
    /// Maximum concurrent kicad-cli jobs (default 2).
    pub concurrency: Option<usize>,
    /// Quiet period after the last source change before building (default 1500).
    pub debounce_ms: Option<u64>,
    /// Build revisions kept in the cache per project (default 5).
    pub keep_revisions: Option<usize>,
    /// Per-command timeout in seconds (default 300; STEP 900).
    pub timeout_seconds: Option<u64>,
    /// Per-stage command timeout overrides in seconds, keyed by stage id.
    pub stage_timeouts: Option<BTreeMap<String, u64>>,
    /// Enabled stage ids (default all).
    pub stages: Option<Vec<String>>,
}

pub(crate) fn validate_build_config(config: &Option<BuildConfig>) -> Result<()> {
    let Some(config) = config else { return Ok(()) };
    if let Some(value) = config.concurrency {
        if !(1..=16).contains(&value) {
            return Err(anyhow!("build.concurrency must be between 1 and 16"));
        }
    }
    if let Some(value) = config.debounce_ms {
        if !(100..=60_000).contains(&value) {
            return Err(anyhow!("build.debounceMs must be between 100 and 60000"));
        }
    }
    if config.keep_revisions == Some(0) {
        return Err(anyhow!("build.keepRevisions must be at least 1"));
    }
    if config.timeout_seconds == Some(0) {
        return Err(anyhow!("build.timeoutSeconds must be at least 1"));
    }
    for (id, seconds) in config.stage_timeouts.iter().flatten() {
        if Stage::from_id(id).is_none() {
            return Err(anyhow!("build.stageTimeouts has unknown stage {id:?}"));
        }
        if *seconds == 0 {
            return Err(anyhow!("build.stageTimeouts.{id} must be at least 1"));
        }
    }
    for id in config.stages.iter().flatten() {
        if Stage::from_id(id).is_none() {
            return Err(anyhow!(
                "build.stages has unknown stage {id:?}; expected one of {}",
                Stage::ALL.map(Stage::id).join(", ")
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct BuildSettings {
    pub auto: bool,
    pub concurrency: usize,
    pub debounce: Duration,
    pub keep_revisions: usize,
    pub timeouts: HashMap<Stage, Duration>,
    pub stages: Vec<Stage>,
}

impl BuildSettings {
    pub(crate) fn from_config(config: Option<&BuildConfig>) -> Self {
        let default = BuildConfig::default();
        let config = config.unwrap_or(&default);
        let timeouts = Stage::ALL
            .into_iter()
            .map(|stage| {
                let seconds = config
                    .stage_timeouts
                    .as_ref()
                    .and_then(|map| map.get(stage.id()).copied())
                    .or(config.timeout_seconds.filter(|_| stage != Stage::Step));
                (
                    stage,
                    seconds
                        .map(Duration::from_secs)
                        .unwrap_or_else(|| stage.default_timeout()),
                )
            })
            .collect();
        let stages = match &config.stages {
            Some(ids) => Stage::ALL
                .into_iter()
                .filter(|stage| ids.iter().any(|id| id == stage.id()))
                .collect(),
            None => Stage::ALL.to_vec(),
        };
        Self {
            auto: config.auto.unwrap_or(true),
            concurrency: config.concurrency.unwrap_or(2),
            debounce: Duration::from_millis(config.debounce_ms.unwrap_or(1500)),
            keep_revisions: config.keep_revisions.unwrap_or(5),
            timeouts,
            stages,
        }
    }
}

// ---------------------------------------------------------------------------
// Command runner with task-local timeout and cancellation

#[derive(Clone)]
pub(crate) struct CommandCtx {
    pub timeout: Duration,
    pub cancel: watch::Receiver<bool>,
}

tokio::task_local! {
    static COMMAND_CTX: CommandCtx;
}

#[derive(Debug)]
pub(crate) struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled: superseded by a newer revision")
    }
}
impl std::error::Error for Cancelled {}

/// Runs an external command. Inside a build job the command inherits the
/// job's timeout and cancellation; the child is killed if either fires.
pub(crate) async fn run_command(
    program: &str,
    args: &[OsString],
    cwd: &Path,
) -> Result<CommandOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().with_context(|| format!("run {program}"))?;
    let wait = child.wait_with_output();
    let output = match COMMAND_CTX.try_with(Clone::clone).ok() {
        None => wait.await?,
        Some(mut ctx) => {
            tokio::select! {
                output = wait => output?,
                _ = tokio::time::sleep(ctx.timeout) => {
                    return Err(anyhow!("{program} timed out after {}s", ctx.timeout.as_secs()));
                }
                _ = wait_cancelled(&mut ctx.cancel) => return Err(Cancelled.into()),
            }
        }
    };
    Ok(CommandOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            // Sender dropped without cancelling: never fire.
            std::future::pending::<()>().await;
        }
    }
}

// ---------------------------------------------------------------------------
// Stage keys and on-disk results

pub(crate) fn stage_key(
    stage: Stage,
    project: &ProjectContext,
    hashes: &SourceHashes,
    kicad_version: &str,
    kct_version: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"kicad-pcb-build/v1\0");
    digest.update(stage.id().as_bytes());
    digest.update([0]);
    digest.update(kicad_version.as_bytes());
    digest.update([0]);
    digest.update(project.config.root.to_string_lossy().as_bytes());
    digest.update([0]);
    digest.update(
        serde_json::to_string(&project.config.kicad)
            .unwrap_or_default()
            .as_bytes(),
    );
    let (sch, pcb) = stage.deps();
    if sch {
        digest.update(b"\0sch\0");
        digest.update(hashes.sch.as_bytes());
    }
    if pcb {
        digest.update(b"\0pcb\0");
        digest.update(hashes.pcb.as_bytes());
    }
    if stage == Stage::Quality {
        digest.update(b"\0quality\0");
        digest.update(crate::quality::AUDIT_VERSION.as_bytes());
        digest.update([0]);
        digest.update(kct_version.as_bytes());
        digest.update([0]);
        digest.update(crate::quality::profile_sha(project).as_bytes());
        digest.update([0]);
        digest.update(
            serde_json::to_string(&project.quality.config)
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    if stage == Stage::Jlcpcb {
        if let Some(path) = artifacts::placement_offsets_path(project) {
            digest.update(b"\0offsets\0");
            digest.update(revision::file_sha256(&path).unwrap_or_default().as_bytes());
        }
    }
    revision::hex(&digest.finalize())
}

pub(crate) fn stage_dir_name(stage: Stage, key: &str) -> String {
    format!("{}-{}", stage.id(), &key[..32.min(key.len())])
}

pub(crate) fn builds_dir(project: &ProjectContext) -> Result<PathBuf> {
    Ok(crate::cache_dir_for(&project.root)?.join("builds"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StageResult {
    pub stage: String,
    pub key: String,
    pub ok: bool,
    pub message: Option<String>,
    pub started_ms: u64,
    pub finished_ms: u64,
    pub elapsed_ms: u64,
    pub outputs: Vec<String>,
    pub kicad_version: String,
    pub log: String,
}

pub(crate) fn read_result(dir: &Path) -> Option<StageResult> {
    let text = std::fs::read_to_string(dir.join("result.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Output of one stage execution.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub ok: bool,
    pub message: Option<String>,
    pub log: String,
}

// ---------------------------------------------------------------------------
// Scheduler (pure bookkeeping, unit tested)

#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub key: String,
    pub stage: Stage,
    pub project: String,
    priority: u8,
    seq: u64,
}

#[derive(Debug)]
struct Running {
    project: String,
    cancel: watch::Sender<bool>,
}

#[derive(Debug, Clone, Default)]
struct ProjectBuild {
    hashes: SourceHashes,
    keys: BTreeMap<Stage, String>,
    stages: BTreeMap<Stage, StageStatus>,
    publish: PublishStatus,
}

#[derive(Debug, Default)]
pub(crate) struct Scheduler {
    projects: HashMap<String, ProjectBuild>,
    queue: Vec<Job>,
    running: HashMap<String, Running>,
    debounce: HashMap<String, u64>,
    seq: u64,
}

impl Scheduler {
    fn wanted(&self) -> HashSet<String> {
        self.projects
            .values()
            .flat_map(|build| build.keys.values().cloned())
            .collect()
    }

    fn is_pending(&self, key: &str) -> bool {
        self.running.contains_key(key) || self.queue.iter().any(|job| job.key == key)
    }

    /// Queues a job unless an identical input key is already queued or
    /// running. Returns true when newly queued.
    pub(crate) fn enqueue(&mut self, project: &str, stage: Stage, key: &str) -> bool {
        if self.is_pending(key) {
            return false;
        }
        self.seq += 1;
        self.queue.push(Job {
            key: key.to_string(),
            stage,
            project: project.to_string(),
            priority: stage.priority(),
            seq: self.seq,
        });
        true
    }

    /// Records the stage keys a project currently wants.
    pub(crate) fn set_wanted(&mut self, project: &str, keys: BTreeMap<Stage, String>) {
        self.projects.entry(project.to_string()).or_default().keys = keys;
    }

    /// Drops queued jobs and cancels running jobs no project wants anymore.
    /// Returns (dropped queued keys, cancelled running keys).
    pub(crate) fn supersede(&mut self) -> (Vec<String>, Vec<String>) {
        let wanted = self.wanted();
        let mut dropped = Vec::new();
        self.queue.retain(|job| {
            let keep = wanted.contains(&job.key);
            if !keep {
                dropped.push(job.key.clone());
            }
            keep
        });
        let mut cancelled = Vec::new();
        for (key, running) in &self.running {
            if !wanted.contains(key) {
                let _ = running.cancel.send(true);
                cancelled.push(key.clone());
            }
        }
        (dropped, cancelled)
    }

    /// Pops the highest-priority wanted job and marks it running.
    pub(crate) fn start_next(&mut self) -> Option<(Job, watch::Receiver<bool>)> {
        let wanted = self.wanted();
        self.queue.retain(|job| wanted.contains(&job.key));
        let index = self
            .queue
            .iter()
            .enumerate()
            .max_by_key(|(_, job)| (job.priority, std::cmp::Reverse(job.seq)))
            .map(|(index, _)| index)?;
        let job = self.queue.remove(index);
        let (cancel, rx) = watch::channel(false);
        self.running.insert(
            job.key.clone(),
            Running {
                project: job.project.clone(),
                cancel,
            },
        );
        Some((job, rx))
    }

    fn finish(&mut self, key: &str) {
        self.running.remove(key);
    }

    fn project_busy(&self, project: &str) -> bool {
        self.queue.iter().any(|job| job.project == project)
            || self.running.values().any(|job| job.project == project)
    }

    fn pending_keys(&self, project: &str) -> Vec<String> {
        self.queue
            .iter()
            .filter(|job| job.project == project)
            .map(|job| job.key.clone())
            .chain(
                self.running
                    .iter()
                    .filter(|(_, job)| job.project == project)
                    .map(|(key, _)| key.clone()),
            )
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Engine

#[derive(Clone)]
pub(crate) struct BuildEngine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for BuildEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildEngine")
            .field("settings", &self.inner.settings)
            .finish_non_exhaustive()
    }
}

struct Inner {
    settings: BuildSettings,
    kicad_version: String,
    kicad_available: bool,
    kct_version: String,
    projects: Vec<ProjectContext>,
    events: broadcast::Sender<ServerEvent>,
    completed: broadcast::Sender<String>,
    sched: Mutex<Scheduler>,
    wake: Notify,
    semaphore: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Refresh status only; uncached stages show as `stale`.
    Status,
    /// Queue uncached stages.
    Build,
    /// Discard cached results and rebuild.
    Force,
}

impl BuildEngine {
    pub(crate) fn new(
        settings: BuildSettings,
        projects: Vec<ProjectContext>,
        kicad_version: Option<String>,
        kct_version: Option<String>,
        events: broadcast::Sender<ServerEvent>,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(settings.concurrency));
        let engine = Self {
            inner: Arc::new(Inner {
                kicad_available: kicad_version.is_some(),
                kicad_version: kicad_version.unwrap_or_else(|| "unavailable".to_string()),
                kct_version: kct_version.unwrap_or_else(|| "unavailable".to_string()),
                settings,
                projects,
                events,
                completed: broadcast::channel(256).0,
                sched: Mutex::new(Scheduler::default()),
                wake: Notify::new(),
                semaphore,
            }),
        };
        let dispatcher = engine.clone();
        tokio::spawn(async move { dispatcher.dispatch().await });
        engine
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Scheduler> {
        self.inner.sched.lock().expect("build scheduler poisoned")
    }

    fn project(&self, id: &str) -> Option<ProjectContext> {
        self.inner
            .projects
            .iter()
            .find(|project| project.id == id)
            .cloned()
    }

    /// Called when a project's sources may have changed. Marks affected
    /// stages stale immediately and schedules a debounced build.
    pub(crate) fn request(&self, project: &ProjectContext) {
        let engine = self.clone();
        let project = project.clone();
        let generation = {
            let mut sched = self.lock();
            let generation = sched.debounce.entry(project.id.clone()).or_default();
            *generation += 1;
            *generation
        };
        tokio::spawn(async move {
            if let Err(err) = engine.plan(&project, Mode::Status).await {
                tracing::warn!(error = %err, project = %project.id, "build status refresh failed");
            }
            if !engine.inner.settings.auto {
                return;
            }
            tokio::time::sleep(engine.inner.settings.debounce).await;
            let current = engine.lock().debounce.get(&project.id).copied();
            if current != Some(generation) {
                return;
            }
            if let Err(err) = engine.plan(&project, Mode::Build).await {
                tracing::warn!(error = %err, project = %project.id, "build planning failed");
            }
        });
    }

    /// Explicit build trigger (API). `force` discards cached results.
    pub(crate) async fn run(&self, project: &ProjectContext, force: bool) -> Result<BuildStatus> {
        self.plan(project, if force { Mode::Force } else { Mode::Build })
            .await?;
        Ok(self.status(&project.id))
    }

    /// Re-reads publish staleness (e.g. after fab outputs changed on disk).
    pub(crate) async fn refresh_status(&self, project: &ProjectContext) {
        if let Err(err) = self.plan(project, Mode::Status).await {
            tracing::debug!(error = %err, "build status refresh failed");
        }
    }

    async fn hashes(project: &ProjectContext) -> Result<ProjectHashes> {
        let project = project.clone();
        tokio::task::spawn_blocking(move || revision::project_hashes(&project)).await?
    }

    async fn plan(&self, project: &ProjectContext, mode: Mode) -> Result<()> {
        let hashes = Self::hashes(project).await?;
        let dir = builds_dir(project)?;
        let settings = &self.inner.settings;
        let mut keys = BTreeMap::new();
        let mut results = BTreeMap::new();
        for stage in &settings.stages {
            let key = stage_key(
                *stage,
                project,
                &hashes.hashes,
                &self.inner.kicad_version,
                &self.inner.kct_version,
            );
            let stage_dir = dir.join(stage_dir_name(*stage, &key));
            if mode == Mode::Force && !self.lock().running.contains_key(&key) {
                let _ = std::fs::remove_dir_all(&stage_dir);
            }
            if let Some(result) = read_result(&stage_dir) {
                results.insert(*stage, result);
            }
            keys.insert(*stage, key);
        }

        let publish = {
            let keys = keys.clone();
            let project = project.clone();
            let files = hashes.files.clone();
            tokio::task::spawn_blocking(move || artifacts::staleness(&project, &files, &keys))
                .await?
        };

        let mut queued_any = false;
        let changed = {
            let mut sched = self.lock();
            let previous = sched.projects.get(&project.id).cloned();
            sched.set_wanted(&project.id, keys.clone());
            let mut stages = BTreeMap::new();
            for (stage, key) in &keys {
                let prior = previous
                    .as_ref()
                    .and_then(|build| build.stages.get(stage))
                    .filter(|status| &status.input_key == key);
                let status = if let Some(result) = results.get(stage) {
                    status_from_result(*stage, result, prior.map(|p| !p.cached).unwrap_or(true))
                } else if !self.inner.kicad_available && stage.needs_kicad() {
                    idle_status(*stage, key, "failed", Some("kicad-cli is required".into()))
                } else if sched.running.contains_key(key) {
                    prior
                        .cloned()
                        .unwrap_or_else(|| idle_status(*stage, key, "running", None))
                } else if sched.queue.iter().any(|job| &job.key == key) {
                    idle_status(*stage, key, "queued", None)
                } else if mode == Mode::Status {
                    // Keep a terminal non-cached state (timeout/cancelled) visible.
                    match prior {
                        Some(prior) if prior.state == "failed" || prior.state == "cancelled" => {
                            prior.clone()
                        }
                        _ => idle_status(*stage, key, "stale", None),
                    }
                } else {
                    sched.enqueue(&project.id, *stage, key);
                    queued_any = true;
                    idle_status(*stage, key, "queued", None)
                };
                stages.insert(*stage, status);
            }
            let changed = previous.map(|prev| (prev.stages, prev.publish, prev.hashes))
                != Some((stages.clone(), publish.clone(), hashes.hashes.clone()));
            let build = sched.projects.entry(project.id.clone()).or_default();
            build.hashes = hashes.hashes.clone();
            build.stages = stages;
            build.publish = publish;
            sched.supersede();
            changed
        };
        if queued_any {
            self.inner.wake.notify_one();
        }
        if mode != Mode::Status {
            let dirs = keys
                .iter()
                .map(|(stage, key)| stage_dir_name(*stage, key))
                .collect();
            let revision = hashes.hashes.revision.clone();
            let dir = dir.clone();
            let _ = tokio::task::spawn_blocking(move || touch_history(&dir, &revision, dirs)).await;
        }
        if changed {
            self.emit(&project.id);
        }
        Ok(())
    }

    fn emit(&self, project: &str) {
        let status = self.status(project);
        let _ = self.inner.events.send(ServerEvent::Build {
            project: project.to_string(),
            status,
        });
    }

    pub(crate) fn status(&self, project: &str) -> BuildStatus {
        let sched = self.lock();
        let auto_publish = self
            .project(project)
            .map(|project| artifacts::auto_publish(&project))
            .unwrap_or(false);
        let Some(build) = sched.projects.get(project) else {
            return BuildStatus {
                project: project.to_string(),
                auto_publish,
                updated_ms: crate::now_ms(),
                ..BuildStatus::default()
            };
        };
        BuildStatus {
            project: project.to_string(),
            hashes: build.hashes.clone(),
            busy: sched.project_busy(project),
            auto_publish,
            stages: build.stages.values().cloned().collect(),
            publish: build.publish.clone(),
            updated_ms: crate::now_ms(),
        }
    }

    /// Current status of a project, planning it first if it has not been seen.
    pub(crate) async fn current_status(&self, project: &ProjectContext) -> Result<BuildStatus> {
        let known = self.lock().projects.contains_key(&project.id);
        if !known {
            self.plan(project, Mode::Status).await?;
        }
        Ok(self.status(&project.id))
    }

    /// Returns the cached result directory for a stage at the current
    /// revision, building it now (bypassing debounce) if needed.
    pub(crate) async fn ensure(
        &self,
        project: &ProjectContext,
        stage: Stage,
    ) -> Result<(StageResult, PathBuf)> {
        if !self.inner.settings.stages.contains(&stage) {
            return Err(anyhow!("build stage {} is disabled", stage.id()));
        }
        for _ in 0..3 {
            let mut completed = self.inner.completed.subscribe();
            self.plan(project, Mode::Build).await?;
            let key = self
                .lock()
                .projects
                .get(&project.id)
                .and_then(|build| build.keys.get(&stage).cloned())
                .ok_or_else(|| anyhow!("stage {} not planned", stage.id()))?;
            let dir = builds_dir(project)?.join(stage_dir_name(stage, &key));
            if let Some(result) = read_result(&dir) {
                return Ok((result, dir));
            }
            if !self.inner.kicad_available && stage.needs_kicad() {
                return Err(anyhow!("kicad-cli is required"));
            }
            let limit = self.inner.settings.timeouts[&stage] * 4 + Duration::from_secs(60);
            let wait = async {
                loop {
                    match completed.recv().await {
                        Ok(done) if done == key => break,
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            if !self.lock().is_pending(&key) {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            };
            if tokio::time::timeout(limit, wait).await.is_err() {
                return Err(anyhow!("timed out waiting for {} build", stage.id()));
            }
            if let Some(result) = read_result(&dir) {
                return Ok((result, dir));
            }
            let status = self
                .lock()
                .projects
                .get(&project.id)
                .and_then(|build| build.stages.get(&stage).cloned());
            if let Some(status) = status {
                if status.input_key == key && status.state == "failed" {
                    return Err(anyhow!(
                        "{} failed: {}",
                        stage.label(),
                        status.message.unwrap_or_default()
                    ));
                }
            }
            // Cancelled or superseded: re-plan against the new revision.
        }
        Err(anyhow!(
            "{} kept being superseded; try again",
            stage.label()
        ))
    }

    /// DRC/ERC result for the current revision, served from the build cache.
    pub(crate) async fn check(
        &self,
        project: &ProjectContext,
        stage: Stage,
    ) -> Result<kicad_pcb_shared::CheckResponse> {
        let uncached = (!self.inner.kicad_available && stage.needs_kicad())
            || !self.inner.settings.stages.contains(&stage);
        if uncached {
            return if stage == Stage::Quality {
                crate::quality::run_check(project).await
            } else {
                crate::run_check(project, stage.id()).await
            };
        }
        let (_, dir) = self.ensure(project, stage).await?;
        let text = tokio::fs::read_to_string(dir.join("check.json"))
            .await
            .context("cached check result")?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Result directory of a finished stage at the project's current planned
    /// revision, without building.
    pub(crate) fn current_result(
        &self,
        project: &ProjectContext,
        stage: Stage,
    ) -> Option<(StageResult, PathBuf)> {
        let key = self
            .lock()
            .projects
            .get(&project.id)?
            .keys
            .get(&stage)?
            .clone();
        let dir = builds_dir(project).ok()?.join(stage_dir_name(stage, &key));
        read_result(&dir).map(|result| (result, dir))
    }

    /// Current planned results for every stage, if the project is idle.
    pub(crate) fn complete_build(
        &self,
        project: &ProjectContext,
    ) -> Result<(SourceHashes, BuildSnapshot)> {
        let (hashes, keys) = {
            let sched = self.lock();
            if sched.project_busy(&project.id) {
                return Err(anyhow!("build is still running; publish when it finishes"));
            }
            let build = sched
                .projects
                .get(&project.id)
                .ok_or_else(|| anyhow!("project has not been built yet"))?;
            (build.hashes.clone(), build.keys.clone())
        };
        let dir = builds_dir(project)?;
        let stages = keys
            .into_iter()
            .map(|(stage, key)| {
                let stage_dir = dir.join(stage_dir_name(stage, &key));
                (stage, (key, read_result(&stage_dir), stage_dir))
            })
            .collect();
        Ok((hashes, stages))
    }

    async fn dispatch(self) {
        loop {
            let Ok(permit) = self.inner.semaphore.clone().acquire_owned().await else {
                return;
            };
            let next = loop {
                let started = self.lock().start_next();
                if let Some(next) = started {
                    break next;
                }
                self.inner.wake.notified().await;
            };
            let engine = self.clone();
            tokio::spawn(async move { engine.run_job(next.0, next.1, permit).await });
        }
    }

    fn update_stage(
        &self,
        project: &str,
        stage: Stage,
        key: &str,
        update: impl FnOnce(&mut StageStatus),
    ) {
        let mut sched = self.lock();
        if let Some(status) = sched
            .projects
            .get_mut(project)
            .and_then(|build| build.stages.get_mut(&stage))
            .filter(|status| status.input_key == key)
        {
            update(status);
        }
    }

    async fn run_job(&self, job: Job, cancel: watch::Receiver<bool>, permit: OwnedSemaphorePermit) {
        let started_ms = crate::now_ms();
        self.update_stage(&job.project, job.stage, &job.key, |status| {
            status.state = "running".into();
            status.started_ms = Some(started_ms);
            status.message = None;
        });
        self.emit(&job.project);

        let outcome = match self.project(&job.project) {
            Some(project) => self.execute_job(&project, &job, cancel, started_ms).await,
            None => Err(anyhow!("unknown project {}", job.project)),
        };
        let finished_ms = crate::now_ms();
        let elapsed = finished_ms.saturating_sub(started_ms);
        match &outcome {
            Ok(JobEnd::Done(result)) => {
                let result = result.clone();
                self.update_stage(&job.project, job.stage, &job.key, move |status| {
                    *status = status_from_result(job.stage, &result, true);
                });
            }
            Ok(JobEnd::Superseded) => {
                self.update_stage(&job.project, job.stage, &job.key, |status| {
                    status.state = "stale".into();
                    status.message = Some("sources changed during build".into());
                    status.elapsed_ms = Some(elapsed);
                });
            }
            Err(err) => {
                let cancelled = err.downcast_ref::<Cancelled>().is_some();
                let message = format!("{err:#}");
                self.update_stage(&job.project, job.stage, &job.key, |status| {
                    status.state = if cancelled { "cancelled" } else { "failed" }.into();
                    status.message = Some(message);
                    status.finished_ms = Some(finished_ms);
                    status.elapsed_ms = Some(elapsed);
                });
            }
        }
        let idle = {
            let mut sched = self.lock();
            sched.finish(&job.key);
            !sched.project_busy(&job.project)
        };
        drop(permit);
        let _ = self.inner.completed.send(job.key.clone());
        if idle {
            if let Some(project) = self.project(&job.project) {
                self.after_build(&project).await;
            }
        }
        self.emit(&job.project);
    }

    async fn execute_job(
        &self,
        project: &ProjectContext,
        job: &Job,
        cancel: watch::Receiver<bool>,
        started_ms: u64,
    ) -> Result<JobEnd> {
        let dir = builds_dir(project)?;
        let final_dir = dir.join(stage_dir_name(job.stage, &job.key));
        let tmp = dir.join(format!(
            ".tmp-{}-{}-{}",
            stage_dir_name(job.stage, &job.key),
            std::process::id(),
            job.seq
        ));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await?;
        let ctx = CommandCtx {
            timeout: self.inner.settings.timeouts[&job.stage],
            cancel,
        };
        let clock = Instant::now();
        let executed = COMMAND_CTX
            .scope(ctx, artifacts::execute(job.stage, project, &tmp))
            .await;
        let elapsed_ms = clock.elapsed().as_millis() as u64;
        let outcome = match executed {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = tokio::fs::remove_dir_all(&tmp).await;
                return Err(err);
            }
        };
        // Guard against sources changing mid-build: the outputs must match the
        // key they are stored under.
        let verify = Self::hashes(project).await?;
        let verify_key = stage_key(
            job.stage,
            project,
            &verify.hashes,
            &self.inner.kicad_version,
            &self.inner.kct_version,
        );
        if verify_key != job.key {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            return Ok(JobEnd::Superseded);
        }
        let outputs = list_outputs(&tmp);
        let result = StageResult {
            stage: job.stage.id().to_string(),
            key: job.key.clone(),
            ok: outcome.ok,
            message: outcome.message,
            started_ms,
            finished_ms: crate::now_ms(),
            elapsed_ms,
            outputs,
            kicad_version: self.inner.kicad_version.clone(),
            log: tail_text(&outcome.log, 8000),
        };
        tokio::fs::write(tmp.join("result.json"), serde_json::to_vec_pretty(&result)?).await?;
        if final_dir.exists() {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
        } else {
            tokio::fs::rename(&tmp, &final_dir).await?;
        }
        Ok(JobEnd::Done(result))
    }

    async fn after_build(&self, project: &ProjectContext) {
        if artifacts::auto_publish(project) {
            match self.publish(project).await {
                Ok(_) => tracing::info!(project = %project.id, "auto-published build"),
                Err(err) => {
                    tracing::info!(project = %project.id, error = %err, "auto-publish skipped")
                }
            }
        }
        let protected = self.lock().pending_keys(&project.id);
        let keep = self.inner.settings.keep_revisions;
        let gc_target = project.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(err) = gc_project(&gc_target, keep, &protected) {
                tracing::warn!(error = %err, project = %gc_target.id, "build cache GC failed");
            }
        })
        .await;
        // Refresh staleness after publish/GC.
        self.refresh_status(project).await;
    }

    /// Copies the current build into the project's artifact dirs.
    pub(crate) async fn publish(&self, project: &ProjectContext) -> Result<serde_json::Value> {
        let (hashes, stages) = self.complete_build(project)?;
        let current = Self::hashes(project).await?;
        if current.hashes != hashes {
            return Err(anyhow!(
                "sources changed since the last build; wait for the rebuild"
            ));
        }
        let project_clone = project.clone();
        let kicad = self.inner.kicad_version.clone();
        let files = current.files.clone();
        let report = tokio::task::spawn_blocking(move || {
            artifacts::publish(&project_clone, &hashes, &files, &stages, &kicad)
        })
        .await??;
        self.plan(project, Mode::Status).await?;
        Ok(report)
    }
}

/// Stage -> (input key, finished result, cache dir) for one revision.
pub(crate) type BuildSnapshot = BTreeMap<Stage, (String, Option<StageResult>, PathBuf)>;

enum JobEnd {
    Done(StageResult),
    Superseded,
}

fn status_from_result(stage: Stage, result: &StageResult, fresh: bool) -> StageStatus {
    StageStatus {
        stage: stage.id().to_string(),
        label: stage.label().to_string(),
        state: if result.ok { "ok" } else { "failed" }.to_string(),
        input_key: result.key.clone(),
        cached: !fresh,
        started_ms: Some(result.started_ms),
        finished_ms: Some(result.finished_ms),
        elapsed_ms: Some(result.elapsed_ms),
        message: result.message.clone(),
        outputs: result.outputs.clone(),
    }
}

fn idle_status(stage: Stage, key: &str, state: &str, message: Option<String>) -> StageStatus {
    StageStatus {
        stage: stage.id().to_string(),
        label: stage.label().to_string(),
        state: state.to_string(),
        input_key: key.to_string(),
        cached: false,
        started_ms: None,
        finished_ms: None,
        elapsed_ms: None,
        message,
        outputs: Vec::new(),
    }
}

fn list_outputs(dir: &Path) -> Vec<String> {
    let mut outputs = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "result.json")
        .collect::<Vec<_>>();
    outputs.sort();
    outputs
}

pub(crate) fn tail_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

// ---------------------------------------------------------------------------
// Cache GC

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HistoryEntry {
    pub revision: String,
    pub dirs: Vec<String>,
    pub last_used_ms: u64,
}

fn history_path(builds: &Path) -> PathBuf {
    builds.join("history.json")
}

fn read_history(builds: &Path) -> Vec<HistoryEntry> {
    std::fs::read_to_string(history_path(builds))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_history(builds: &Path, history: &[HistoryEntry]) -> Result<()> {
    std::fs::create_dir_all(builds)?;
    let tmp = builds.join(format!(".history-{}.json", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(history)?)?;
    std::fs::rename(tmp, history_path(builds))?;
    Ok(())
}

fn touch_history(builds: &Path, revision: &str, dirs: Vec<String>) -> Result<()> {
    let mut history = read_history(builds);
    history.retain(|entry| entry.revision != revision);
    history.push(HistoryEntry {
        revision: revision.to_string(),
        dirs,
        last_used_ms: crate::now_ms(),
    });
    write_history(builds, &history)
}

/// Keeps the `keep` most recently used revisions plus `protected` stage dirs;
/// deletes every other stage dir. Returns removed dir names.
pub(crate) fn gc_builds(
    builds: &Path,
    history: &mut Vec<HistoryEntry>,
    keep: usize,
    protected: &HashSet<String>,
) -> Result<Vec<String>> {
    history.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_ms));
    history.truncate(keep);
    let mut keep_dirs: HashSet<String> = history
        .iter()
        .flat_map(|entry| entry.dirs.iter().cloned())
        .collect();
    keep_dirs.extend(protected.iter().cloned());
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(builds) else {
        return Ok(removed);
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if name.starts_with(".tmp-") {
            // Abandoned temp dirs from crashed builds.
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(6 * 3600));
            if old {
                let _ = std::fs::remove_dir_all(&path);
                removed.push(name);
            }
            continue;
        }
        if !keep_dirs.contains(&name) {
            std::fs::remove_dir_all(&path)?;
            removed.push(name);
        }
    }
    removed.sort();
    Ok(removed)
}

fn gc_project(project: &ProjectContext, keep: usize, pending_keys: &[String]) -> Result<()> {
    let builds = builds_dir(project)?;
    let mut history = read_history(&builds);
    let mut protected: HashSet<String> = artifacts::published_stage_dirs(project);
    for key in pending_keys {
        for stage in Stage::ALL {
            protected.insert(stage_dir_name(stage, key));
        }
    }
    let removed = gc_builds(&builds, &mut history, keep, &protected)?;
    write_history(&builds, &history)?;
    // Pre-pipeline GLB cache files (<cache>/<revision>.glb) are never reused.
    if let Some(cache) = builds.parent() {
        for entry in std::fs::read_dir(cache).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|v| v.to_str()) == Some("glb") {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    if !removed.is_empty() {
        tracing::info!(project = %project.id, removed = removed.len(), "build cache GC");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP routes

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/build/status", get(status_endpoint))
        .route("/api/build/run", post(run_endpoint))
        .route("/api/build/publish", post(publish_endpoint))
        .route("/api/build/events", get(sse_endpoint))
        .route("/api/build/artifact", get(artifact_endpoint))
        .route("/api/kicad/gerbers", get(artifacts::gerbers_endpoint))
        .route("/kicad-pcb/build-strip.js", get(build_strip_js))
}

#[derive(Debug, Deserialize)]
struct BuildQuery {
    project: Option<String>,
    force: Option<String>,
}

async fn status_endpoint(
    State(state): State<AppState>,
    Query(query): Query<BuildQuery>,
) -> Result<Json<BuildStatus>, AppError> {
    let project = crate::selected_project(&state, query.project.as_deref())?;
    Ok(Json(state.builds.current_status(&project).await?))
}

async fn run_endpoint(
    State(state): State<AppState>,
    Query(query): Query<BuildQuery>,
) -> Result<Json<BuildStatus>, AppError> {
    let project = crate::selected_project(&state, query.project.as_deref())?;
    let force = query
        .force
        .as_deref()
        .is_some_and(|value| !matches!(value, "" | "0" | "false"));
    Ok(Json(state.builds.run(&project, force).await?))
}

async fn publish_endpoint(
    State(state): State<AppState>,
    Query(query): Query<BuildQuery>,
) -> Response {
    let project = match crate::selected_project(&state, query.project.as_deref()) {
        Ok(project) => project,
        Err(err) => return AppError(err).into_response(),
    };
    match state.builds.publish(&project).await {
        Ok(report) => Json(report).into_response(),
        Err(err) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"ok": false, "message": format!("{err:#}")})),
        )
            .into_response(),
    }
}

async fn sse_endpoint(
    State(state): State<AppState>,
    Query(query): Query<BuildQuery>,
) -> Result<impl IntoResponse, AppError> {
    let project = crate::selected_project(&state, query.project.as_deref())?;
    let initial = state.builds.current_status(&project).await?;
    let receiver = state.events.subscribe();
    let id = project.id.clone();
    let first = futures::stream::once(async move {
        Ok::<_, std::convert::Infallible>(
            SseEvent::default()
                .event("Build")
                .json_data(ServerEvent::Build {
                    project: initial.project.clone(),
                    status: initial,
                })
                .unwrap_or_default(),
        )
    });
    let rest = futures::stream::unfold((receiver, id), |(mut receiver, id)| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if !event_for_project(&event, &id) {
                        continue;
                    }
                    let name = match &event {
                        ServerEvent::Build { .. } => "Build",
                        ServerEvent::Revision { .. } => "Revision",
                    };
                    let item = SseEvent::default()
                        .event(name)
                        .json_data(&event)
                        .unwrap_or_default();
                    return Some((Ok(item), (receiver, id)));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    use futures::StreamExt;
    Ok(Sse::new(first.chain(rest)).keep_alive(KeepAlive::default()))
}

/// True when an event should be delivered to a client watching `project`.
pub(crate) fn event_for_project(event: &ServerEvent, project: &str) -> bool {
    match event {
        ServerEvent::Revision { project: None, .. } => true,
        ServerEvent::Revision {
            project: Some(id), ..
        } => id == project,
        ServerEvent::Build { project: id, .. } => id == project,
    }
}

#[derive(Debug, Deserialize)]
struct ArtifactQuery {
    project: Option<String>,
    stage: String,
    file: String,
    download: Option<String>,
}

async fn artifact_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ArtifactQuery>,
) -> Result<Response, AppError> {
    let project = crate::selected_project(&state, query.project.as_deref())?;
    let stage = Stage::from_id(&query.stage).ok_or_else(|| anyhow!("unknown stage"))?;
    if query.file.contains(['/', '\\']) || query.file.starts_with('.') {
        return Err(anyhow!("invalid artifact name").into());
    }
    let Some((result, dir)) = state.builds.current_result(&project, stage) else {
        return Ok((StatusCode::NOT_FOUND, "stage has no current build").into_response());
    };
    if !result.outputs.iter().any(|name| name == &query.file) {
        return Ok((StatusCode::NOT_FOUND, "artifact not in build").into_response());
    }
    let path = dir.join(&query.file);
    let bytes = tokio::fs::read(&path).await?;
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    let mut response = ([(header::CONTENT_TYPE, mime)], bytes).into_response();
    if query.download.is_some() {
        let value = format!("attachment; filename=\"{}\"", query.file);
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&value).map_err(|err| anyhow!(err))?,
        );
    }
    Ok(response)
}

async fn build_strip_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        crate::artifacts::BUILD_STRIP_JS,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn keys(pairs: &[(Stage, &str)]) -> BTreeMap<Stage, String> {
        pairs
            .iter()
            .map(|(stage, key)| (*stage, key.to_string()))
            .collect()
    }

    #[test]
    fn enqueue_dedupes_identical_input_keys() {
        let mut sched = Scheduler::default();
        sched.set_wanted("a", keys(&[(Stage::Drc, "k1")]));
        assert!(sched.enqueue("a", Stage::Drc, "k1"));
        assert!(!sched.enqueue("a", Stage::Drc, "k1"));
        let (job, _) = sched.start_next().unwrap();
        assert_eq!(job.key, "k1");
        assert!(!sched.enqueue("a", Stage::Drc, "k1"), "running key dedupes");
        sched.finish("k1");
        assert!(sched.enqueue("a", Stage::Drc, "k1"));
    }

    #[test]
    fn new_revision_supersedes_queued_and_cancels_running() {
        let mut sched = Scheduler::default();
        sched.set_wanted(
            "a",
            keys(&[
                (Stage::Glb, "glb1"),
                (Stage::Step, "step1"),
                (Stage::Erc, "erc1"),
            ]),
        );
        for (stage, key) in [
            (Stage::Glb, "glb1"),
            (Stage::Step, "step1"),
            (Stage::Erc, "erc1"),
        ] {
            sched.enqueue("a", stage, key);
        }
        let (running, cancel) = sched.start_next().unwrap();
        assert_eq!(running.stage, Stage::Glb, "highest priority first");

        // A PCB-only edit: GLB/STEP keys change, ERC (sch) key is unchanged.
        sched.set_wanted(
            "a",
            keys(&[
                (Stage::Glb, "glb2"),
                (Stage::Step, "step2"),
                (Stage::Erc, "erc1"),
            ]),
        );
        let (dropped, cancelled) = sched.supersede();
        assert_eq!(dropped, vec!["step1".to_string()]);
        assert_eq!(cancelled, vec!["glb1".to_string()]);
        assert!(*cancel.borrow());
        assert!(sched.queue.iter().any(|job| job.key == "erc1"));
    }

    #[test]
    fn keys_are_shared_across_projects_wanting_them() {
        let mut sched = Scheduler::default();
        sched.set_wanted("a", keys(&[(Stage::Erc, "k")]));
        sched.set_wanted("b", keys(&[(Stage::Erc, "k")]));
        sched.enqueue("a", Stage::Erc, "k");
        sched.set_wanted("a", keys(&[(Stage::Erc, "other")]));
        let (dropped, _) = sched.supersede();
        assert!(dropped.is_empty(), "still wanted by b");
    }

    #[test]
    fn gc_keeps_recent_and_published_revisions() {
        let tmp = TempDir::new().unwrap();
        let builds = tmp.path();
        let mut history = Vec::new();
        for index in 0..8u64 {
            let dir = format!("gerbers-{index:032}");
            std::fs::create_dir_all(builds.join(&dir)).unwrap();
            history.push(HistoryEntry {
                revision: format!("r{index}"),
                dirs: vec![dir],
                last_used_ms: index,
            });
        }
        let published = format!("gerbers-{:032}", 1);
        let protected = HashSet::from([published.clone()]);
        let removed = gc_builds(builds, &mut history, 3, &protected).unwrap();
        assert_eq!(history.len(), 3);
        let remaining: HashSet<String> = std::fs::read_dir(builds)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            remaining.contains(&published),
            "published revision survives"
        );
        for index in 5..8 {
            assert!(remaining.contains(&format!("gerbers-{index:032}")));
        }
        assert_eq!(removed.len(), 4);
    }

    #[test]
    fn stage_keys_depend_only_on_their_inputs() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        let project = crate::revision::tests_support::project(&repo, "a", ".");
        let base = SourceHashes {
            revision: "r".into(),
            sch: "s1".into(),
            pcb: "p1".into(),
        };
        let pcb_edit = SourceHashes {
            pcb: "p2".into(),
            ..base.clone()
        };
        let key =
            |stage, hashes: &SourceHashes| stage_key(stage, &project, hashes, "10.0.6", "kct");
        assert_eq!(key(Stage::Erc, &base), key(Stage::Erc, &pcb_edit));
        assert_ne!(key(Stage::Drc, &base), key(Stage::Drc, &pcb_edit));
        assert_ne!(key(Stage::Glb, &base), key(Stage::Glb, &pcb_edit));
        assert_ne!(
            stage_key(Stage::Erc, &project, &base, "10.0.6", "kct"),
            stage_key(Stage::Erc, &project, &base, "10.0.7", "kct"),
            "KiCad upgrades invalidate outputs"
        );
        assert_ne!(key(Stage::Quality, &base), key(Stage::Quality, &pcb_edit));
        let sch_edit = SourceHashes {
            sch: "s2".into(),
            ..base.clone()
        };
        assert_ne!(key(Stage::Quality, &base), key(Stage::Quality, &sch_edit));
        assert_ne!(
            stage_key(
                Stage::Quality,
                &project,
                &base,
                "10.0.6",
                "kicad-tools 0.21.1"
            ),
            stage_key(
                Stage::Quality,
                &project,
                &base,
                "10.0.6",
                "kicad-tools 0.22.0"
            ),
            "kct upgrades re-run quality checks"
        );
        assert_eq!(
            stage_key(Stage::Erc, &project, &base, "10.0.6", "a"),
            stage_key(Stage::Erc, &project, &base, "10.0.6", "b"),
            "kct version only affects the quality stage"
        );
    }

    #[test]
    fn build_config_validation() {
        assert!(validate_build_config(&Some(BuildConfig {
            concurrency: Some(0),
            ..Default::default()
        }))
        .is_err());
        assert!(validate_build_config(&Some(BuildConfig {
            stages: Some(vec!["gerbers".into(), "nope".into()]),
            ..Default::default()
        }))
        .is_err());
        let ok = BuildConfig {
            stages: Some(vec!["drc".into(), "step".into()]),
            stage_timeouts: Some(BTreeMap::from([("step".to_string(), 60)])),
            ..Default::default()
        };
        assert!(validate_build_config(&Some(ok.clone())).is_ok());
        let settings = BuildSettings::from_config(Some(&ok));
        assert_eq!(settings.stages, vec![Stage::Drc, Stage::Step]);
        assert_eq!(settings.timeouts[&Stage::Step], Duration::from_secs(60));
        assert_eq!(settings.debounce, Duration::from_millis(1500));
        assert_eq!(settings.concurrency, 2);
    }
}
