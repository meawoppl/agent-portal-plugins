//! Content-addressed thumbnail renders for the library view.
//!
//! Every render is described by a [`RenderSpec`]: a self-contained KiCad input
//! file (throwaway `.kicad_sym`, `.kicad_mod` or `.kicad_pcb`), the model files it
//! references, and the render kind. The cache key hashes all of that together
//! with the kicad-cli version, the render parameters and [`RENDERER_VERSION`],
//! so only items whose content actually changed get re-rendered.
//!
//! Rendering runs on a small bounded worker pool ([`Renderer`]). The pool is
//! intentionally self-contained: `enqueue`/`prioritize`/`render_now` form the
//! seam a shared job queue can replace later without touching the library view.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{process::Command, sync::Notify};

/// Bump when render inputs/outputs change shape so stale thumbnails are ignored.
pub const RENDERER_VERSION: &str = "library-thumbs-v4";
const RENDER_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_WORKERS: usize = 2;
const DEFAULT_CACHE_MB: u64 = 256;
const GC_EVERY_RENDERS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderKind {
    Symbol,
    Footprint,
    Model,
    Glb,
}

impl RenderKind {
    pub fn extension(self) -> &'static str {
        match self {
            RenderKind::Symbol | RenderKind::Footprint => "svg",
            RenderKind::Model => "png",
            RenderKind::Glb => "glb",
        }
    }

    /// Render parameters that participate in the cache key.
    pub fn params(self) -> &'static [&'static str] {
        match self {
            RenderKind::Symbol => &["sym-export-svg", "palette=tokyo-night"],
            RenderKind::Footprint => &[
                "fp-export-svg",
                "--layers",
                FOOTPRINT_LAYERS,
                "palette=tokyo-night",
            ],
            RenderKind::Model => &[
                "pcb-render",
                "--width",
                "320",
                "--height",
                "240",
                "--rotate",
                "-50,0,35",
                "--background",
                "transparent",
                "--quality",
                "basic",
            ],
            RenderKind::Glb => &["pcb-export-glb", "--subst-models"],
        }
    }
}

const FOOTPRINT_LAYERS: &str = "F.Cu,B.Cu,F.Paste,F.SilkS,F.Fab,F.CrtYd,Edge.Cuts";

/// A fully self-contained render request.
#[derive(Debug, Clone)]
pub struct RenderSpec {
    pub kind: RenderKind,
    /// Item name inside `input` (symbol or footprint name); unused for PCB renders.
    pub item: String,
    /// Complete file contents handed to kicad-cli.
    pub input: String,
    /// Optional normalized identity used only for cache-key generation. The
    /// renderer still receives `input`; this lets equivalent generated inputs
    /// share expensive renders while retaining real local paths for KiCad.
    pub cache_identity: Option<String>,
    /// Additional CLI render/export arguments, typically derived from geometry.
    pub params: Vec<String>,
    /// Model files referenced by `input`; their bytes participate in the key.
    pub model_files: Vec<PathBuf>,
}

impl RenderSpec {
    pub fn key(&self, kicad_version: &str) -> String {
        let model_hashes = self
            .model_files
            .iter()
            .map(|path| (path.display().to_string(), file_digest(path)))
            .collect::<Vec<_>>();
        cache_key(
            self.kind,
            &self.item,
            self.cache_identity.as_deref().unwrap_or(&self.input),
            &model_hashes,
            &self.params,
            kicad_version,
        )
    }
}

/// sha256 over every input that can change the rendered pixels.
pub fn cache_key(
    kind: RenderKind,
    item: &str,
    input: &str,
    model_hashes: &[(String, Option<String>)],
    extra_params: &[String],
    kicad_version: &str,
) -> String {
    let mut digest = Sha256::new();
    let mut field = |label: &str, value: &[u8]| {
        digest.update(label.as_bytes());
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    };
    field("renderer", RENDERER_VERSION.as_bytes());
    field("kicad", kicad_version.as_bytes());
    field("kind", kind.extension().as_bytes());
    field("kind-name", format!("{kind:?}").as_bytes());
    for param in kind.params() {
        field("param", param.as_bytes());
    }
    for param in extra_params {
        field("extra-param", param.as_bytes());
    }
    field("item", item.as_bytes());
    field("input", input.as_bytes());
    for (path, hash) in model_hashes {
        field("model", path.as_bytes());
        field(
            "model-hash",
            hash.as_deref().unwrap_or("missing").as_bytes(),
        );
    }
    let hex = format!("{:x}", digest.finalize());
    hex[..32].to_string()
}

/// sha256 of a file, memoized by (path, size, mtime) so repeated inventory
/// requests do not rehash large STEP files.
pub fn file_digest(path: &Path) -> Option<String> {
    type Memo = Mutex<HashMap<PathBuf, (u64, SystemTime, String)>>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let memo = MEMO.get_or_init(Default::default);
    if let Some((size, mtime, hash)) = memo.lock().ok()?.get(path) {
        if *size == meta.len() && *mtime == modified {
            return Some(hash.clone());
        }
    }
    let bytes = std::fs::read(path).ok()?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    memo.lock()
        .ok()?
        .insert(path.to_path_buf(), (meta.len(), modified, hash.clone()));
    Some(hash)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum JobState {
    /// Registered but not requested (on-demand outputs such as GLB).
    Idle,
    Queued,
    Rendering,
    Ready {
        ms: Option<u64>,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct RenderStats {
    pub rendered: usize,
    pub failed: usize,
    pub cache_hits: usize,
    pub total_render_ms: u64,
    pub queued: usize,
    pub rendering: usize,
}

struct Inner {
    queue: VecDeque<String>,
    specs: HashMap<String, Arc<RenderSpec>>,
    states: HashMap<String, JobState>,
    stats: RenderStats,
    renders_since_gc: usize,
}

/// Bounded background render pool writing into `dir`.
pub struct Renderer {
    dir: PathBuf,
    kicad_cli: Option<String>,
    kicad_version: String,
    inner: Mutex<Inner>,
    wake: Notify,
    done: Notify,
    cache_cap_bytes: u64,
}

impl Renderer {
    /// Shared renderer for a cache directory (one per repo cache root).
    pub fn for_dir(dir: PathBuf, kicad_cli: Option<String>, kicad_version: String) -> Arc<Self> {
        type Registry = Mutex<HashMap<PathBuf, Arc<Renderer>>>;
        static RENDERERS: OnceLock<Registry> = OnceLock::new();
        let registry = RENDERERS.get_or_init(Default::default);
        let mut registry = registry.lock().expect("renderer registry poisoned");
        if let Some(existing) = registry.get(&dir) {
            return existing.clone();
        }
        let workers = std::env::var("KICAD_PCB_LIBRARY_WORKERS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value: &usize| *value > 0)
            .unwrap_or(DEFAULT_WORKERS);
        let cache_mb = std::env::var("KICAD_PCB_LIBRARY_CACHE_MB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_CACHE_MB);
        let renderer = Arc::new(Renderer {
            dir: dir.clone(),
            kicad_cli,
            kicad_version,
            inner: Mutex::new(Inner {
                queue: VecDeque::new(),
                specs: HashMap::new(),
                states: HashMap::new(),
                stats: RenderStats::default(),
                renders_since_gc: 0,
            }),
            wake: Notify::new(),
            done: Notify::new(),
            cache_cap_bytes: cache_mb * 1024 * 1024,
        });
        let _ = std::fs::create_dir_all(&dir);
        let _ = gc_dir(&dir, renderer.cache_cap_bytes);
        for _ in 0..workers {
            let worker = renderer.clone();
            tokio::spawn(async move { worker.worker_loop().await });
        }
        registry.insert(dir, renderer.clone());
        renderer
    }

    pub fn kicad_version(&self) -> &str {
        &self.kicad_version
    }

    pub fn output_path(&self, key: &str, kind: RenderKind) -> PathBuf {
        self.dir.join(format!("{key}.{}", kind.extension()))
    }

    /// Register a spec, returning its key and current state. Specs already on
    /// disk are reported ready without queueing.
    pub fn register(&self, spec: RenderSpec, enqueue: bool) -> (String, JobState) {
        let key = spec.key(&self.kicad_version);
        let path = self.output_path(&key, spec.kind);
        let mut inner = self.inner.lock().expect("renderer poisoned");
        inner
            .specs
            .entry(key.clone())
            .or_insert_with(|| Arc::new(spec));
        if let Some(state) = inner.states.get(&key) {
            let requeue = match state {
                // Output was garbage-collected: render again.
                JobState::Ready { .. } => !path.exists(),
                JobState::Idle => enqueue,
                _ => false,
            };
            if !requeue {
                return (key.clone(), state.clone());
            }
        }
        if path.exists() {
            inner.stats.cache_hits += 1;
            inner
                .states
                .insert(key.clone(), JobState::Ready { ms: None });
            return (key, JobState::Ready { ms: None });
        }
        if !enqueue {
            inner.states.insert(key.clone(), JobState::Idle);
            return (key, JobState::Idle);
        }
        inner.states.insert(key.clone(), JobState::Queued);
        inner.queue.push_back(key.clone());
        drop(inner);
        self.wake.notify_one();
        (key, JobState::Queued)
    }

    pub fn spec(&self, key: &str) -> Option<Arc<RenderSpec>> {
        self.inner.lock().ok()?.specs.get(key).cloned()
    }

    pub fn state(&self, key: &str) -> Option<JobState> {
        self.inner.lock().ok()?.states.get(key).cloned()
    }

    pub fn stats(&self) -> RenderStats {
        let inner = self.inner.lock().expect("renderer poisoned");
        let mut stats = inner.stats.clone();
        stats.queued = inner.queue.len();
        stats.rendering = inner
            .states
            .values()
            .filter(|state| matches!(state, JobState::Rendering))
            .count();
        stats
    }

    /// Move queued keys to the front, in the given order (first = most urgent).
    pub fn prioritize(&self, keys: &[String]) {
        let mut inner = self.inner.lock().expect("renderer poisoned");
        let wanted = keys.iter().collect::<HashSet<_>>();
        let mut front = Vec::new();
        inner.queue.retain(|key| {
            if wanted.contains(key) {
                front.push(key.clone());
                false
            } else {
                true
            }
        });
        let order = keys
            .iter()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect::<HashMap<_, _>>();
        front.sort_by_key(|key| order.get(key).copied().unwrap_or(usize::MAX));
        for key in front.into_iter().rev() {
            inner.queue.push_front(key);
        }
    }

    /// Queue (at the front) and wait for a registered key to finish.
    pub async fn render_now(&self, key: &str) -> Result<PathBuf> {
        let spec = self
            .spec(key)
            .ok_or_else(|| anyhow!("unknown render key {key}"))?;
        let path = self.output_path(key, spec.kind);
        if path.exists() {
            return Ok(path);
        }
        {
            let mut inner = self.inner.lock().expect("renderer poisoned");
            if !matches!(inner.states.get(key), Some(JobState::Rendering)) {
                inner.queue.retain(|item| item != key);
                inner.queue.push_front(key.to_string());
                inner.states.insert(key.to_string(), JobState::Queued);
            }
        }
        self.wake.notify_one();
        let deadline = Instant::now() + RENDER_TIMEOUT + Duration::from_secs(30);
        loop {
            let notified = self.done.notified();
            match self.state(key) {
                Some(JobState::Ready { .. }) if path.exists() => return Ok(path),
                Some(JobState::Failed { message }) => return Err(anyhow!(message)),
                _ => {}
            }
            if Instant::now() > deadline {
                return Err(anyhow!("timed out waiting for render {key}"));
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), notified).await;
        }
    }

    /// Mark an output as recently used so GC keeps it.
    pub fn touch(&self, path: &Path) {
        if let Ok(file) = std::fs::File::options().append(true).open(path) {
            let _ = file.set_modified(SystemTime::now());
        }
    }

    async fn worker_loop(self: Arc<Self>) {
        loop {
            let next = {
                let mut inner = self.inner.lock().expect("renderer poisoned");
                let key = inner.queue.pop_front();
                if let Some(key) = &key {
                    inner.states.insert(key.clone(), JobState::Rendering);
                }
                key.and_then(|key| inner.specs.get(&key).cloned().map(|spec| (key, spec)))
            };
            let Some((key, spec)) = next else {
                self.wake.notified().await;
                continue;
            };
            let started = Instant::now();
            let output = self.output_path(&key, spec.kind);
            let result = if output.exists() {
                Ok(())
            } else {
                render_spec(self.kicad_cli.as_deref(), &spec, &output).await
            };
            let elapsed = started.elapsed().as_millis() as u64;
            let run_gc = {
                let mut inner = self.inner.lock().expect("renderer poisoned");
                match result {
                    Ok(()) => {
                        inner.stats.rendered += 1;
                        inner.stats.total_render_ms += elapsed;
                        inner
                            .states
                            .insert(key.clone(), JobState::Ready { ms: Some(elapsed) });
                    }
                    Err(err) => {
                        tracing::warn!(%key, kind = ?spec.kind, item = %spec.item, "library render failed: {err:#}");
                        inner.stats.failed += 1;
                        inner.states.insert(
                            key.clone(),
                            JobState::Failed {
                                message: format!("{err:#}"),
                            },
                        );
                    }
                }
                inner.renders_since_gc += 1;
                let due = inner.renders_since_gc >= GC_EVERY_RENDERS;
                if due {
                    inner.renders_since_gc = 0;
                }
                due
            };
            self.done.notify_waiters();
            if run_gc {
                let dir = self.dir.clone();
                let cap = self.cache_cap_bytes;
                let _ = tokio::task::spawn_blocking(move || gc_dir(&dir, cap)).await;
            }
        }
    }
}

/// Run kicad-cli for one spec, writing the final artifact atomically to `output`.
pub async fn render_spec(kicad_cli: Option<&str>, spec: &RenderSpec, output: &Path) -> Result<()> {
    let cli = kicad_cli.ok_or_else(|| anyhow!("kicad-cli not found"))?;
    let work = tempfile::Builder::new()
        .prefix("kicad-pcb-library-")
        .tempdir()
        .context("create render tempdir")?;
    let out_dir = work.path().join("out");
    std::fs::create_dir_all(&out_dir)?;
    let produced = match spec.kind {
        RenderKind::Symbol => {
            let lib = work.path().join("item.kicad_sym");
            std::fs::write(&lib, &spec.input)?;
            run_cli(
                cli,
                &[
                    "sym".into(),
                    "export".into(),
                    "svg".into(),
                    "--symbol".into(),
                    spec.item.clone(),
                    "-o".into(),
                    out_dir.display().to_string(),
                    lib.display().to_string(),
                ],
                work.path(),
            )
            .await?;
            let svg = first_output(&out_dir, "svg")?;
            let text = std::fs::read_to_string(&svg)?;
            recolor_svg(&text, SYMBOL_PALETTE).into_bytes()
        }
        RenderKind::Footprint => {
            let pretty = work.path().join("item.pretty");
            std::fs::create_dir_all(&pretty)?;
            std::fs::write(
                pretty.join(format!("{}.kicad_mod", sanitize(&spec.item))),
                &spec.input,
            )?;
            run_cli(
                cli,
                &[
                    "fp".into(),
                    "export".into(),
                    "svg".into(),
                    "--footprint".into(),
                    spec.item.clone(),
                    "--layers".into(),
                    FOOTPRINT_LAYERS.into(),
                    "-o".into(),
                    out_dir.display().to_string(),
                    pretty.display().to_string(),
                ],
                work.path(),
            )
            .await?;
            let svg = first_output(&out_dir, "svg")?;
            let text = std::fs::read_to_string(&svg)?;
            recolor_svg(&text, FOOTPRINT_PALETTE).into_bytes()
        }
        RenderKind::Model => {
            let pcb = work.path().join("item.kicad_pcb");
            std::fs::write(&pcb, &spec.input)?;
            let png = out_dir.join("item.png");
            let mut args = vec![
                "pcb".to_string(),
                "render".to_string(),
                "-o".to_string(),
                png.display().to_string(),
            ];
            args.extend(spec.kind.params()[1..].iter().map(|v| v.to_string()));
            args.extend(spec.params.iter().cloned());
            args.push(pcb.display().to_string());
            run_cli(cli, &args, work.path()).await?;
            std::fs::read(&png).context("read rendered png")?
        }
        RenderKind::Glb => {
            let pcb = work.path().join("item.kicad_pcb");
            std::fs::write(&pcb, &spec.input)?;
            let glb = out_dir.join("item.glb");
            let mut args = vec![
                "pcb".to_string(),
                "export".to_string(),
                "glb".to_string(),
                "-o".to_string(),
                glb.display().to_string(),
            ];
            args.extend(spec.kind.params()[1..].iter().map(|v| v.to_string()));
            args.extend(spec.params.iter().cloned());
            args.push(pcb.display().to_string());
            run_cli(cli, &args, work.path()).await?;
            std::fs::read(&glb).context("read exported glb")?
        }
    };
    if produced.is_empty() {
        return Err(anyhow!("kicad-cli produced an empty file"));
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = output.with_extension(format!("{}.tmp", spec.kind.extension()));
    std::fs::write(&staging, produced)?;
    std::fs::rename(&staging, output)?;
    Ok(())
}

async fn run_cli(cli: &str, args: &[String], cwd: &Path) -> Result<()> {
    let child = Command::new(cli)
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(RENDER_TIMEOUT, child)
        .await
        .map_err(|_| anyhow!("kicad-cli {} timed out", args[..2].join(" ")))?
        .with_context(|| format!("run {cli}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr
            .lines()
            .chain(stdout.lines())
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or("no output")
            .to_string();
        return Err(anyhow!(
            "kicad-cli {} failed ({}): {detail}",
            args[..2].join(" "),
            output.status
        ));
    }
    Ok(())
}

fn first_output(dir: &Path, extension: &str) -> Result<PathBuf> {
    let mut files = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|v| v.to_str()) == Some(extension))
        .collect::<Vec<_>>();
    // Multi-unit symbols export one file per unit; unit 1 sorts first.
    files.sort();
    files
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("kicad-cli produced no .{extension} output"))
}

/// Filename-safe stand-in used for the throwaway `.kicad_mod` file name.
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '+') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Default KiCad plot colors mapped onto a Tokyo Night friendly palette so the
/// transparent SVGs read on the dark (#1a1b26) card background.
const SYMBOL_PALETTE: &[(&str, &str)] = &[
    ("#840000", "#f7768e"), // body outline
    ("#A90000", "#ff9e64"), // pins
    ("#006464", "#7dcfff"), // pin names / fields
    ("#FFFFC2", "#24283b"), // body background fill
    ("#000000", "#c0caf5"), // default text / strokes
];
const FOOTPRINT_PALETTE: &[(&str, &str)] = &[("#000000", "#c0caf5")];

pub fn recolor_svg(svg: &str, palette: &[(&str, &str)]) -> String {
    let mut out = svg.to_string();
    for (from, to) in palette {
        out = out.replace(from, to);
        out = out.replace(&from.to_ascii_lowercase(), to);
    }
    out
}

/// Size-capped GC: delete least-recently-used outputs until the directory is
/// under 80% of `cap_bytes`.
pub fn gc_dir(dir: &Path, cap_bytes: u64) -> Result<usize> {
    let mut entries = Vec::new();
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        total += meta.len();
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        entries.push((modified, meta.len(), entry.path()));
    }
    if total <= cap_bytes {
        return Ok(0);
    }
    entries.sort();
    let target = cap_bytes / 10 * 8;
    let mut removed = 0;
    for (_, size, path) in entries {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_sensitive() {
        let models = vec![("/m/a.step".to_string(), Some("abc".to_string()))];
        let a = cache_key(RenderKind::Model, "X", "(fp)", &models, &[], "10.0.6");
        let b = cache_key(RenderKind::Model, "X", "(fp)", &models, &[], "10.0.6");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        // Pinned so accidental key-format changes are caught; bump
        // RENDERER_VERSION (and this value) when the format intentionally changes.
        assert_eq!(
            cache_key(
                RenderKind::Symbol,
                "R",
                "(symbol \"R\")",
                &[],
                &[],
                "10.0.6"
            ),
            "55f740afac2576c666e6a96d7e71db96"
        );
        let other_model = vec![("/m/a.step".to_string(), Some("abd".to_string()))];
        let missing_model = vec![("/m/a.step".to_string(), None)];
        assert_ne!(
            a,
            cache_key(RenderKind::Model, "X", "(fp)", &other_model, &[], "10.0.6")
        );
        assert_ne!(
            a,
            cache_key(
                RenderKind::Model,
                "X",
                "(fp)",
                &missing_model,
                &[],
                "10.0.6"
            )
        );
        assert_ne!(
            a,
            cache_key(RenderKind::Model, "X", "(fp) ", &models, &[], "10.0.6")
        );
        assert_ne!(
            a,
            cache_key(RenderKind::Model, "X", "(fp)", &models, &[], "10.0.7")
        );
        assert_ne!(
            a,
            cache_key(RenderKind::Glb, "X", "(fp)", &models, &[], "10.0.6")
        );
        assert_ne!(
            cache_key(RenderKind::Symbol, "X", "(fp)", &[], &[], "10.0.6"),
            cache_key(RenderKind::Footprint, "X", "(fp)", &[], &[], "10.0.6")
        );
    }

    #[test]
    fn spec_key_tracks_model_file_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("part.step");
        std::fs::write(&model, b"one").unwrap();
        let spec = RenderSpec {
            kind: RenderKind::Model,
            item: String::new(),
            input: "(kicad_pcb)".to_string(),
            cache_identity: None,
            params: Vec::new(),
            model_files: vec![model.clone()],
        };
        let first = spec.key("10");
        assert_eq!(first, spec.key("10"));
        std::fs::write(&model, b"two!").unwrap();
        assert_ne!(first, spec.key("10"));
    }

    #[test]
    fn cache_identity_can_alias_different_generated_inputs() {
        let a = RenderSpec {
            kind: RenderKind::Model,
            item: String::new(),
            input: "(kicad_pcb (footprint \"A\"))".to_string(),
            cache_identity: Some("same-model".to_string()),
            params: Vec::new(),
            model_files: Vec::new(),
        };
        let b = RenderSpec {
            kind: RenderKind::Model,
            item: String::new(),
            input: "(kicad_pcb (footprint \"B\"))".to_string(),
            cache_identity: Some("same-model".to_string()),
            params: Vec::new(),
            model_files: Vec::new(),
        };
        assert_eq!(a.key("10"), b.key("10"));
        let mut zoomed = b.clone();
        zoomed.params = vec!["--zoom".into(), "0.5".into()];
        assert_ne!(a.key("10"), zoomed.key("10"));
    }

    #[test]
    fn gc_removes_oldest_until_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        for (index, name) in ["a", "b", "c", "d"].iter().enumerate() {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            let file = std::fs::File::options().append(true).open(&path).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64 * 10))
                .unwrap();
        }
        let removed = gc_dir(dir.path(), 250).unwrap();
        assert_eq!(removed, 2);
        assert!(!dir.path().join("a").exists());
        assert!(!dir.path().join("b").exists());
        assert!(dir.path().join("d").exists());
        assert_eq!(gc_dir(dir.path(), 250).unwrap(), 0);
    }

    #[test]
    fn recolors_default_palette() {
        let svg = "stroke:#840000; fill:#FFFFC2";
        assert_eq!(
            recolor_svg(svg, SYMBOL_PALETTE),
            "stroke:#f7768e; fill:#24283b"
        );
    }
}
