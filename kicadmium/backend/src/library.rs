//! Smart library view: part inventory, drift checks and cached renders.
//!
//! The inventory is built from the project schematic (including hierarchical
//! sheets), the PCB, and the project/global KiCad library tables. Each unique
//! (symbol, footprint, model set) becomes a card with a symbol | footprint | 3D
//! triptych rendered lazily by [`crate::thumbnails::Renderer`].

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Instant, SystemTime},
};

use anyhow::{Context, Result};
use axum::{
    extract::{Path as UrlPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use shared::library::{
    LibraryBadge, LibraryPart, LibraryResponse, LibrarySource, LibraryStats, LibraryStatusResponse,
    ModelRef, PartThumbs, ThumbRef, ThumbStatus,
};

use crate::{
    sexp::{self, Sexp},
    thumbnails::{file_digest, JobState, RenderKind, RenderSpec, Renderer},
    AppError, AppState, LibraryConfig, ProjectContext, ProjectQuery,
};

/// Fallback KiCad 10 layer table for throwaway boards when the project PCB is
/// unavailable.
const FALLBACK_LAYERS: &str = r#"(layers (0 "F.Cu" signal) (2 "B.Cu" signal) (9 "F.Adhes" user "F.Adhesive") (11 "B.Adhes" user "B.Adhesive") (13 "F.Paste" user) (15 "B.Paste" user) (5 "F.SilkS" user "F.Silkscreen") (7 "B.SilkS" user "B.Silkscreen") (1 "F.Mask" user) (3 "B.Mask" user) (25 "Edge.Cuts" user) (27 "Margin" user) (31 "F.CrtYd" user "F.Courtyard") (29 "B.CrtYd" user "B.Courtyard") (35 "F.Fab" user) (33 "B.Fab" user))"#;
const FALLBACK_PCB_VERSION: &str = "20260206";
const FALLBACK_SYM_VERSION: &str = "20241209";
/// Thin board so the part, not the substrate, dominates the 3D thumbnail.
const THUMB_BOARD_THICKNESS: &str = "0.4";
const MAX_VIEWER_SOURCES: usize = 8192;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/kicad/library", get(inventory_endpoint))
        .route("/api/kicad/library/status", get(status_endpoint))
        .route("/api/kicad/library/thumb/:file", get(thumb_endpoint))
        .route("/api/kicad/library/source/:file", get(source_endpoint))
        .route("/api/kicad/library/model/:file", get(model_endpoint))
}

// ---------------------------------------------------------------------------
// HTTP handlers

async fn renderer_for(state: &AppState) -> Result<Arc<Renderer>> {
    static VERSION: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    let cli = crate::kicad_cli();
    let version = VERSION
        .get_or_init(|| async {
            match crate::tool_version(crate::kicad_cli()).await {
                Some(version) => version,
                None => "unknown".to_string(),
            }
        })
        .await
        .clone();
    let dir = crate::cache_dir_for(&state.cwd)?.join("library");
    Ok(Renderer::for_dir(dir, cli, version))
}

async fn inventory_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<LibraryResponse>, AppError> {
    let project = crate::selected_project(&state, query.project.as_deref())?;
    let renderer = renderer_for(&state).await?;
    let repo_root = state.cwd.clone();
    let started = Instant::now();
    let inventory =
        tokio::task::spawn_blocking(move || build_inventory(&repo_root, &project)).await??;
    let response = register_inventory(&renderer, inventory, started);
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    /// Comma separated keys to report on.
    keys: Option<String>,
    /// Comma separated keys currently visible; moved to the front of the queue.
    visible: Option<String>,
}

async fn status_endpoint(
    State(state): State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<LibraryStatusResponse>, AppError> {
    let renderer = renderer_for(&state).await?;
    let split = |value: &Option<String>| {
        value
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter(|key| is_key(key))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let visible = split(&query.visible);
    if !visible.is_empty() {
        renderer.prioritize(&visible);
    }
    let mut states = BTreeMap::new();
    let mut pending = 0;
    for key in split(&query.keys).into_iter().chain(visible) {
        let Some(spec) = renderer.spec(&key) else {
            continue;
        };
        let status = thumb_status(&renderer, &key, spec.kind);
        if matches!(status.state.as_str(), "queued" | "rendering") {
            pending += 1;
        }
        states.insert(key, status);
    }
    let render = renderer.stats();
    Ok(Json(LibraryStatusResponse {
        ok: true,
        states,
        pending,
        stats: LibraryStats {
            pending: render.queued + render.rendering,
            rendered: render.rendered,
            failed: render.failed,
            cache_hits: render.cache_hits,
            total_render_ms: render.total_render_ms,
            ..LibraryStats::default()
        },
    }))
}

fn thumb_status(renderer: &Renderer, key: &str, kind: RenderKind) -> ThumbStatus {
    let state = renderer.state(key).unwrap_or(JobState::Idle);
    let (label, message) = job_label(&state);
    ThumbStatus {
        state: label.to_string(),
        url: (label == "ready").then(|| thumb_url(key, kind)),
        message,
    }
}

fn job_label(state: &JobState) -> (&'static str, Option<String>) {
    match state {
        JobState::Idle => ("idle", None),
        JobState::Queued => ("queued", None),
        JobState::Rendering => ("rendering", None),
        JobState::Ready { .. } => ("ready", None),
        JobState::Failed { message } => ("failed", Some(message.clone())),
    }
}

fn thumb_url(key: &str, kind: RenderKind) -> String {
    format!("/api/kicad/library/thumb/{key}.{}", kind.extension())
}

fn is_key(value: &str) -> bool {
    value.len() == 32 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn split_file(file: &str) -> Option<(&str, &str)> {
    let (key, ext) = file.split_once('.')?;
    is_key(key).then_some((key, ext))
}

async fn thumb_endpoint(
    State(state): State<AppState>,
    UrlPath(file): UrlPath<String>,
) -> Result<Response, AppError> {
    let renderer = renderer_for(&state).await?;
    let Some((key, ext)) = split_file(&file) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(kind) = [RenderKind::Symbol, RenderKind::Footprint, RenderKind::Model]
        .into_iter()
        .find(|kind| kind.extension() == ext)
    else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let path = renderer.output_path(key, kind);
    if !path.exists() {
        // A browser asked for it: it is visible, so render it next.
        renderer.prioritize(&[key.to_string()]);
        return Ok((StatusCode::NOT_FOUND, "thumbnail not rendered yet").into_response());
    }
    renderer.touch(&path);
    let bytes = tokio::fs::read(&path).await?;
    let mime = if ext == "svg" {
        "image/svg+xml"
    } else {
        "image/png"
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

async fn source_endpoint(UrlPath(file): UrlPath<String>) -> Response {
    let Some((key, _)) = split_file(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(source) = viewer_sources()
        .lock()
        .ok()
        .and_then(|sources| sources.get(key).cloned())
    else {
        return (
            StatusCode::NOT_FOUND,
            "unknown library item; reload the library",
        )
            .into_response();
    };
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        source.content,
    )
        .into_response()
}

async fn model_endpoint(
    State(state): State<AppState>,
    UrlPath(file): UrlPath<String>,
) -> Result<Response, AppError> {
    let renderer = renderer_for(&state).await?;
    let Some((key, "glb")) = split_file(&file) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let path = match renderer.render_now(key).await {
        Ok(path) => path,
        Err(err) => {
            return Ok((StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")).into_response())
        }
    };
    renderer.touch(&path);
    let bytes = tokio::fs::read(&path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "model/gltf-binary"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Viewer sources (throwaway schematic / board text for the interactive viewers)

#[derive(Debug, Clone)]
struct ViewerSource {
    content: String,
}

fn viewer_sources() -> &'static Mutex<HashMap<String, ViewerSource>> {
    static SOURCES: OnceLock<Mutex<HashMap<String, ViewerSource>>> = OnceLock::new();
    SOURCES.get_or_init(Default::default)
}

// ---------------------------------------------------------------------------
// Inventory model (pure; no renderer state)

/// A thumbnail that could be rendered, before it is registered with the pool.
#[derive(Debug, Clone)]
struct ThumbDraft {
    spec: Option<RenderSpec>,
    /// Placeholder reason when `spec` is None.
    placeholder: Option<String>,
    source: Option<String>,
    /// Viewer file (`.kicad_sch` / `.kicad_pcb`) served under the thumb key.
    viewer: Option<(&'static str, String)>,
    /// On-demand GLB export for the interactive 3D viewer.
    glb: Option<RenderSpec>,
}

impl ThumbDraft {
    fn none(reason: impl Into<String>) -> Self {
        ThumbDraft {
            spec: None,
            placeholder: Some(reason.into()),
            source: None,
            viewer: None,
            glb: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PartDraft {
    part: LibraryPart,
    symbol: ThumbDraft,
    footprint: ThumbDraft,
    model: ThumbDraft,
}

#[derive(Debug, Default)]
struct Inventory {
    project: String,
    parts: Vec<PartDraft>,
    unused: Vec<PartDraft>,
    libraries: Vec<LibrarySource>,
    warnings: Vec<String>,
}

fn register_inventory(
    renderer: &Renderer,
    inventory: Inventory,
    started: Instant,
) -> LibraryResponse {
    let mut stats = LibraryStats {
        parts: inventory.parts.len(),
        unused: inventory.unused.len(),
        ..LibraryStats::default()
    };
    let register_part = |draft: PartDraft, stats: &mut LibraryStats| {
        let mut part = draft.part;
        part.thumbs = PartThumbs {
            symbol: register_thumb(renderer, draft.symbol, stats),
            footprint: register_thumb(renderer, draft.footprint, stats),
            model: register_thumb(renderer, draft.model, stats),
        };
        part
    };
    let parts = inventory
        .parts
        .into_iter()
        .map(|draft| register_part(draft, &mut stats))
        .collect();
    let unused = inventory
        .unused
        .into_iter()
        .map(|draft| register_part(draft, &mut stats))
        .collect();
    let render = renderer.stats();
    stats.inventory_ms = started.elapsed().as_millis() as u64;
    stats.rendered = render.rendered;
    stats.cache_hits = render.cache_hits;
    stats.total_render_ms = render.total_render_ms;
    LibraryResponse {
        ok: true,
        project: inventory.project,
        kicad_version: Some(renderer.kicad_version().to_string()),
        parts,
        unused,
        libraries: inventory.libraries,
        warnings: inventory.warnings,
        stats,
    }
}

fn register_thumb(renderer: &Renderer, draft: ThumbDraft, stats: &mut LibraryStats) -> ThumbRef {
    let Some(spec) = draft.spec else {
        return ThumbRef {
            state: "none".to_string(),
            message: draft.placeholder,
            ..ThumbRef::default()
        };
    };
    let kind = spec.kind;
    let (key, job) = renderer.register(spec, true);
    stats.thumbs += 1;
    let (label, message) = job_label(&job);
    match label {
        "ready" => stats.ready += 1,
        "failed" => stats.failed += 1,
        _ => stats.pending += 1,
    }
    let mut viewer = None;
    if let Some((extension, content)) = draft.viewer {
        if let Ok(mut sources) = viewer_sources().lock() {
            // Bounded: stale entries only matter until the next inventory load.
            if sources.len() > MAX_VIEWER_SOURCES {
                sources.clear();
            }
            sources.insert(key.clone(), ViewerSource { content });
        }
        viewer = Some(format!("/api/kicad/library/source/{key}.{extension}"));
    }
    if let Some(glb) = draft.glb {
        let (glb_key, _) = renderer.register(glb, false);
        viewer = Some(format!("/api/kicad/library/model/{glb_key}.glb"));
    }
    ThumbRef {
        url: Some(thumb_url(&key, kind)),
        key: Some(key),
        state: label.to_string(),
        message: message.or(draft.placeholder),
        viewer,
        source: draft.source,
    }
}

// ---------------------------------------------------------------------------
// Path / variable resolution

#[derive(Debug, Clone)]
struct Resolver {
    vars: HashMap<String, String>,
    project_root: PathBuf,
}

impl Resolver {
    fn new(project_root: &Path) -> Self {
        let mut vars = HashMap::new();
        let share = kicad_share_dir();
        for version in ["", "6", "7", "8", "9", "10"] {
            for (suffix, sub) in [
                ("3DMODEL_DIR", "3dmodels"),
                ("FOOTPRINT_DIR", "footprints"),
                ("SYMBOL_DIR", "symbols"),
                ("TEMPLATE_DIR", "template"),
            ] {
                let name = if version.is_empty() {
                    format!("KICAD_{suffix}")
                } else {
                    format!("KICAD{version}_{suffix}")
                };
                if let Some(share) = &share {
                    vars.insert(name, share.join(sub).display().to_string());
                }
            }
        }
        for (name, value) in std::env::vars() {
            if name.starts_with("KICAD") {
                vars.insert(name, value);
            }
        }
        vars.insert("KIPRJMOD".to_string(), project_root.display().to_string());
        Resolver {
            vars,
            project_root: project_root.to_path_buf(),
        }
    }

    fn expand(&self, value: &str) -> String {
        expand_vars(value, &self.vars)
    }

    fn path(&self, value: &str) -> PathBuf {
        let expanded = self.expand(value);
        let path = PathBuf::from(&expanded);
        if path.is_absolute() {
            path
        } else {
            self.project_root.join(path)
        }
    }
}

fn kicad_share_dir() -> Option<PathBuf> {
    [
        "/usr/share/kicad",
        "/usr/local/share/kicad",
        "/Applications/KiCad/KiCad.app/Contents/SharedSupport",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_dir())
}

/// Expand `${VAR}` and `$(VAR)` references; unknown variables are left as-is.
fn expand_vars(value: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let close = match tail.chars().next() {
            Some('{') => '}',
            Some('(') => ')',
            _ => {
                out.push('$');
                rest = tail;
                continue;
            }
        };
        let Some(end) = tail.find(close) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &tail[1..end];
        match vars.get(name) {
            Some(replacement) => out.push_str(replacement),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Library tables

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibKind {
    Symbol,
    Footprint,
}

#[derive(Debug, Clone)]
struct LibEntry {
    nickname: String,
    path: PathBuf,
    local: bool,
}

#[derive(Debug, Default)]
struct Libraries {
    symbols: HashMap<String, LibEntry>,
    footprints: HashMap<String, LibEntry>,
    order: Vec<(LibKind, String)>,
}

impl Libraries {
    fn load(
        repo_root: &Path,
        project: &ProjectContext,
        resolver: &Resolver,
    ) -> (Self, Vec<String>) {
        let mut libs = Libraries::default();
        let mut warnings = Vec::new();
        // Global tables first so project entries override them.
        if let Some(config) = kicad_config_dir() {
            for (kind, file) in [
                (LibKind::Symbol, "sym-lib-table"),
                (LibKind::Footprint, "fp-lib-table"),
            ] {
                for entry in read_lib_table(&config.join(file), resolver, 0) {
                    libs.insert(kind, entry, false, repo_root);
                }
            }
        }
        for (kind, file) in [
            (LibKind::Symbol, "sym-lib-table"),
            (LibKind::Footprint, "fp-lib-table"),
        ] {
            for entry in read_lib_table(&project.root.join(file), resolver, 0) {
                libs.insert(kind, entry, true, repo_root);
            }
        }
        let mut configured = Vec::new();
        if project.config.inherit_libraries.unwrap_or(true) {
            configured.push((repo_root.to_path_buf(), project.shared_libraries.clone()));
        }
        if let Some(config) = &project.config.libraries {
            configured.push((project.root.clone(), config.clone()));
        }
        for (root, config) in configured {
            libs.insert_configured(&root, &config, repo_root, &mut warnings);
        }
        (libs, warnings)
    }

    fn insert(&mut self, kind: LibKind, mut entry: LibEntry, from_project: bool, repo_root: &Path) {
        entry.local = from_project && entry.path.starts_with(repo_root);
        let map = match kind {
            LibKind::Symbol => &mut self.symbols,
            LibKind::Footprint => &mut self.footprints,
        };
        if !map.contains_key(&entry.nickname) {
            self.order.push((kind, entry.nickname.clone()));
        }
        map.insert(entry.nickname.clone(), entry);
    }

    fn insert_configured(
        &mut self,
        root: &Path,
        config: &LibraryConfig,
        repo_root: &Path,
        warnings: &mut Vec<String>,
    ) {
        let add = |kind: LibKind, path: PathBuf, this: &mut Self| {
            let Some(nickname) = path.file_stem().map(|v| v.to_string_lossy().into_owned()) else {
                return;
            };
            let exists = match kind {
                LibKind::Symbol => this.symbols.get(&nickname),
                LibKind::Footprint => this.footprints.get(&nickname),
            }
            .is_some_and(|entry| entry.path == path);
            if !exists {
                this.insert(
                    kind,
                    LibEntry {
                        nickname,
                        path,
                        local: true,
                    },
                    true,
                    repo_root,
                );
            }
        };
        for path in config.symbols.iter().flatten() {
            let target = root.join(path);
            if target.is_dir() {
                for file in list_dir(&target, |p| has_ext(p, "kicad_sym")) {
                    add(LibKind::Symbol, file, self);
                }
            } else if target.is_file() {
                add(LibKind::Symbol, target, self);
            } else {
                warnings.push(format!("symbol library {} not found", path.display()));
            }
        }
        for path in config.footprints.iter().flatten() {
            let target = root.join(path);
            if has_ext(&target, "pretty") && target.is_dir() {
                add(LibKind::Footprint, target, self);
            } else if target.is_dir() {
                for dir in list_dir(&target, |p| has_ext(p, "pretty") && p.is_dir()) {
                    add(LibKind::Footprint, dir, self);
                }
            } else {
                warnings.push(format!("footprint library {} not found", path.display()));
            }
        }
    }

    fn sources(&self) -> Vec<LibrarySource> {
        self.order
            .iter()
            .filter_map(|(kind, nick)| {
                let (entry, label) = match kind {
                    LibKind::Symbol => (self.symbols.get(nick)?, "symbol"),
                    LibKind::Footprint => (self.footprints.get(nick)?, "footprint"),
                };
                if !entry.local {
                    return None;
                }
                let items = match kind {
                    LibKind::Symbol => load_symbol_lib(&entry.path)
                        .map(|lib| lib.symbols.len())
                        .unwrap_or(0),
                    LibKind::Footprint => footprint_names(&entry.path).len(),
                };
                Some(LibrarySource {
                    nickname: nick.clone(),
                    kind: label.to_string(),
                    path: entry.path.display().to_string(),
                    local: entry.local,
                    exists: entry.path.exists(),
                    items,
                })
            })
            .collect()
    }
}

fn kicad_config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("KICAD_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })
                .map(|dir| dir.join("kicad"))
        })?;
    for version in ["10.0", "9.0", "8.0"] {
        let dir = base.join(version);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    None
}

fn read_lib_table(path: &Path, resolver: &Resolver, depth: usize) -> Vec<LibEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(table) = sexp::parse(&text) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for lib in table.children("lib") {
        let field = |name: &str| lib.child(name).and_then(|node| node.str_at(1));
        let (Some(nickname), Some(uri)) = (field("name"), field("uri")) else {
            continue;
        };
        if field("disabled").is_some()
            || lib
                .items()
                .iter()
                .any(|item| item == &Sexp::atom("disabled"))
        {
            continue;
        }
        let resolved = resolver.path(&uri);
        if field("type").as_deref() == Some("Table") {
            if depth < 2 {
                entries.extend(read_lib_table(&resolved, resolver, depth + 1));
            }
            continue;
        }
        entries.push(LibEntry {
            nickname,
            path: resolved,
            local: false,
        });
    }
    entries
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().and_then(|v| v.to_str()) == Some(ext)
}

fn list_dir(dir: &Path, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut items = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| keep(path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    items.sort();
    items
}

fn footprint_names(pretty: &Path) -> Vec<String> {
    list_dir(pretty, |path| has_ext(path, "kicad_mod"))
        .into_iter()
        .filter_map(|path| path.file_stem().map(|v| v.to_string_lossy().into_owned()))
        .collect()
}

#[derive(Debug)]
struct SymbolLib {
    version: String,
    symbols: Vec<(String, Sexp)>,
}

impl SymbolLib {
    fn get(&self, name: &str) -> Option<&Sexp> {
        self.symbols
            .iter()
            .find(|(item, _)| item == name)
            .map(|(_, node)| node)
    }

    /// The symbol plus any `extends` parents, parents first.
    fn with_parents(&self, name: &str) -> Option<Vec<Sexp>> {
        let mut chain = Vec::new();
        let mut current = name.to_string();
        let mut seen = HashSet::new();
        while seen.insert(current.clone()) {
            let node = self.get(&current)?;
            chain.push(node.clone());
            match node.child("extends").and_then(|node| node.str_at(1)) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        chain.reverse();
        Some(chain)
    }
}

fn load_symbol_lib(path: &Path) -> Option<Arc<SymbolLib>> {
    type Memo = Mutex<HashMap<PathBuf, (SystemTime, Arc<SymbolLib>)>>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    if let Some((mtime, lib)) = memo.lock().ok()?.get(path) {
        if *mtime == modified {
            return Some(lib.clone());
        }
    }
    let text = std::fs::read_to_string(path).ok()?;
    let root = sexp::parse(&text).ok()?;
    let version = root
        .child("version")
        .and_then(|node| node.str_at(1))
        .unwrap_or_else(|| FALLBACK_SYM_VERSION.to_string());
    let symbols = root
        .children("symbol")
        .filter_map(|node| Some((node.str_at(1)?, node.clone())))
        .collect();
    let lib = Arc::new(SymbolLib { version, symbols });
    memo.lock()
        .ok()?
        .insert(path.to_path_buf(), (modified, lib.clone()));
    Some(lib)
}

fn split_lib_id(lib_id: &str) -> (Option<&str>, &str) {
    match lib_id.split_once(':') {
        Some((nick, name)) => (Some(nick), name),
        None => (None, lib_id),
    }
}

fn library_symbol(libs: &Libraries, lib_id: &str) -> LibraryLookup<(String, Vec<Sexp>)> {
    let (Some(nick), name) = split_lib_id(lib_id) else {
        return LibraryLookup::NoLibrary;
    };
    let Some(entry) = libs.symbols.get(nick) else {
        return LibraryLookup::NoLibrary;
    };
    let Some(lib) = load_symbol_lib(&entry.path) else {
        return LibraryLookup::Missing(entry.path.clone());
    };
    match lib.with_parents(name) {
        Some(chain) => LibraryLookup::Found((lib.version.clone(), chain)),
        None => LibraryLookup::Missing(entry.path.clone()),
    }
}

fn library_footprint(libs: &Libraries, fpid: &str) -> LibraryLookup<(PathBuf, Sexp)> {
    let (Some(nick), name) = split_lib_id(fpid) else {
        return LibraryLookup::NoLibrary;
    };
    let Some(entry) = libs.footprints.get(nick) else {
        return LibraryLookup::NoLibrary;
    };
    let file = entry.path.join(format!("{name}.kicad_mod"));
    match std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| sexp::parse(&text).ok())
    {
        Some(node) => LibraryLookup::Found((file, node)),
        None => LibraryLookup::Missing(entry.path.clone()),
    }
}

enum LibraryLookup<T> {
    Found(T),
    /// Library nickname resolved, item not present.
    Missing(PathBuf),
    /// No such library in any table (or no nickname).
    NoLibrary,
}

impl<T> LibraryLookup<T> {
    fn found(&self) -> Option<&T> {
        match self {
            LibraryLookup::Found(value) => Some(value),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Schematic / PCB parsing

#[derive(Debug, Clone, Default)]
struct SchInstance {
    references: Vec<String>,
    lib_id: String,
    cache_name: String,
    fields: Vec<(String, String)>,
    power: bool,
}

#[derive(Debug, Default)]
struct SchData {
    version: Option<String>,
    lib_symbols: HashMap<String, Sexp>,
    instances: Vec<SchInstance>,
}

fn load_schematic(root_file: &Path) -> Result<SchData> {
    let mut data = SchData::default();
    let mut seen = HashSet::new();
    let mut stack = vec![root_file.to_path_buf()];
    while let Some(file) = stack.pop() {
        let canonical = file.canonicalize().unwrap_or_else(|_| file.clone());
        if !seen.insert(canonical) || seen.len() > 256 {
            continue;
        }
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("read schematic {}", file.display()))?;
        let root = sexp::parse(&text).with_context(|| format!("parse {}", file.display()))?;
        if data.version.is_none() {
            data.version = root.child("version").and_then(|node| node.str_at(1));
        }
        if let Some(cache) = root.child("lib_symbols") {
            for symbol in cache.children("symbol") {
                if let Some(name) = symbol.str_at(1) {
                    data.lib_symbols
                        .entry(name)
                        .or_insert_with(|| symbol.clone());
                }
            }
        }
        for sheet in root.children("sheet") {
            let sheetfile = sheet
                .property("Sheetfile")
                .or_else(|| sheet.property("Sheet file"));
            if let Some(sheetfile) = sheetfile {
                let base = file.parent().unwrap_or(Path::new("."));
                stack.push(base.join(sheetfile));
            }
        }
        data.instances
            .extend(root.children("symbol").filter_map(schematic_instance));
    }
    Ok(data)
}

fn schematic_instance(node: &Sexp) -> Option<SchInstance> {
    let lib_id = node.child("lib_id")?.str_at(1)?;
    let cache_name = node
        .child("lib_name")
        .and_then(|name| name.str_at(1))
        .unwrap_or_else(|| lib_id.clone());
    let fields = node.properties();
    let mut references = BTreeSet::new();
    // Hierarchical instances carry per-sheet-path references.
    if let Some(instances) = node.child("instances") {
        collect_references(instances, &mut references);
    }
    if references.is_empty() {
        if let Some(reference) = node.property("Reference") {
            references.insert(reference);
        }
    }
    let power = references
        .iter()
        .all(|reference| reference.starts_with('#'));
    let references = references
        .into_iter()
        .filter(|reference| !reference.starts_with('#') && !reference.is_empty())
        .collect();
    Some(SchInstance {
        references,
        lib_id,
        cache_name,
        fields,
        power,
    })
}

fn collect_references(node: &Sexp, out: &mut BTreeSet<String>) {
    for item in node.items() {
        if item.is("reference") {
            if let Some(reference) = item.str_at(1) {
                out.insert(reference);
            }
        } else if matches!(item, Sexp::List(_)) {
            collect_references(item, out);
        }
    }
}

#[derive(Debug, Clone)]
struct PcbFootprint {
    reference: String,
    fpid: String,
    node: Sexp,
    fields: Vec<(String, String)>,
    bottom: bool,
}

#[derive(Debug, Default)]
struct PcbData {
    version: Option<String>,
    layers: Option<Sexp>,
    footprints: Vec<PcbFootprint>,
}

fn load_pcb(path: &Path) -> Result<PcbData> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read PCB {}", path.display()))?;
    let root = sexp::parse(&text).with_context(|| format!("parse {}", path.display()))?;
    let footprints = root
        .children("footprint")
        .filter_map(|node| {
            let fpid = node.str_at(1)?;
            let reference = node
                .property("Reference")
                .or_else(|| {
                    node.children("fp_text")
                        .find(|text| text.str_at(1).as_deref() == Some("reference"))
                        .and_then(|text| text.str_at(2))
                })
                .unwrap_or_default();
            let bottom = node
                .child("layer")
                .and_then(|layer| layer.str_at(1))
                .is_some_and(|layer| layer == "B.Cu");
            Some(PcbFootprint {
                reference,
                fpid,
                fields: node.properties(),
                node: node.clone(),
                bottom,
            })
        })
        .collect();
    Ok(PcbData {
        version: root.child("version").and_then(|node| node.str_at(1)),
        layers: root.child("layers").cloned(),
        footprints,
    })
}

// ---------------------------------------------------------------------------
// Normalization, extraction and throwaway file synthesis

/// Replace the first string atom (item name) of a node.
fn rename(node: &mut Sexp, name: &str) {
    if let Some(items) = node.items_mut() {
        if items.len() > 1 {
            items[1] = Sexp::string(name);
        }
    }
}

fn set_property(node: &mut Sexp, name: &str, value: &str) {
    let Some(items) = node.items_mut() else {
        return;
    };
    for item in items.iter_mut() {
        if item.is("property") && item.str_at(1).as_deref() == Some(name) {
            if let Some(fields) = item.items_mut() {
                if fields.len() > 2 {
                    fields[2] = Sexp::string(value);
                }
            }
        }
        if item.is("fp_text") && item.str_at(1).as_deref() == Some(&name.to_ascii_lowercase()) {
            if let Some(fields) = item.items_mut() {
                if fields.len() > 2 {
                    fields[2] = Sexp::string(value);
                }
            }
        }
    }
}

/// Remove descendants whose head is in `heads`, at any depth.
fn strip_heads(node: &mut Sexp, heads: &[&str]) {
    node.walk_mut(&mut |item| {
        item.retain_children(|child| !child.head().is_some_and(|head| heads.contains(&head)));
    });
}

fn at_angle(node: &Sexp) -> f64 {
    node.child("at")
        .and_then(|at| at.str_at(3))
        .and_then(|angle| angle.parse().ok())
        .unwrap_or(0.0)
}

fn format_angle(angle: f64) -> Option<String> {
    let mut angle = angle % 360.0;
    if angle <= -180.0 {
        angle += 360.0;
    }
    if angle > 180.0 {
        angle -= 360.0;
    }
    sexp::normalize_number(&format!("{angle:.4}")).filter(|value| value != "0")
}

/// Undo board placement: move to the origin and subtract the footprint
/// rotation from pad/text angles (board files store those absolutely).
fn unplace(fp: &mut Sexp) {
    let rotation = at_angle(fp);
    if let Some(items) = fp.items_mut() {
        for item in items.iter_mut().skip(1) {
            if item.is("at") {
                *item = sexp::parse("(at 0 0)").expect("static at");
                continue;
            }
            let rotates = ["pad", "fp_text", "property", "fp_text_box", "text_box"]
                .iter()
                .any(|head| item.is(head));
            if !rotates {
                continue;
            }
            let Some(fields) = item.items_mut() else {
                continue;
            };
            for field in fields.iter_mut() {
                if !field.is("at") {
                    continue;
                }
                let Some(at) = field.items_mut() else {
                    continue;
                };
                let angle = at
                    .get(3)
                    .and_then(|value| match value {
                        Sexp::Atom(raw) => raw.parse::<f64>().ok(),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                at.truncate(3);
                if let Some(value) = format_angle(angle - rotation) {
                    at.push(Sexp::atom(value));
                }
                // Preserve a trailing `unlocked` flag if present.
            }
        }
    }
}

/// Heads that only exist because an item is placed in a design.
const PLACEMENT_HEADS: &[&str] = &[
    "uuid",
    "tstamp",
    "net",
    "pinfunction",
    "pintype",
    "path",
    "sheetname",
    "sheetfile",
];

/// Footprint as placed on the board, reduced to a position-independent,
/// render-ready `(footprint ...)` node named `name`.
fn canonical_board_footprint(fp: &Sexp, name: &str) -> Sexp {
    let mut node = fp.clone();
    unplace(&mut node);
    strip_heads(&mut node, PLACEMENT_HEADS);
    node.retain_children(|child| !child.is("locked") && !child.is("placed"));
    rename(&mut node, name);
    set_property(&mut node, "Reference", "REF**");
    set_property(&mut node, "Value", name);
    node
}

/// `.kicad_mod` text for a footprint node (library or canonicalized board copy).
fn footprint_mod_text(fp: &Sexp, name: &str, version: &str) -> String {
    let mut node = fp.clone();
    strip_heads(&mut node, &["uuid", "tstamp"]);
    node.retain_children(|child| {
        !["version", "generator", "generator_version", "at"]
            .iter()
            .any(|head| child.is(head))
    });
    rename(&mut node, name);
    // Thumbnails show geometry; the REF** silk label only adds clutter.
    set_property(&mut node, "Reference", "");
    if let Some(items) = node.items_mut() {
        items.insert(
            2,
            sexp::parse(&format!("(version {version})")).expect("version"),
        );
        items.insert(
            3,
            sexp::parse("(generator \"kicad-pcb-library\")").expect("generator"),
        );
    }
    node.to_string()
}

/// Throwaway `.kicad_pcb` containing one footprint at the origin.
fn footprint_pcb_text(
    fp: &Sexp,
    fpid: &str,
    reference: &str,
    pcb: &PcbData,
    thickness: &str,
) -> String {
    let mut node = fp.clone();
    node.retain_children(|child| {
        !["version", "generator", "generator_version", "at"]
            .iter()
            .any(|head| child.is(head))
    });
    rename(&mut node, fpid);
    set_property(&mut node, "Reference", reference);
    if let Some(items) = node.items_mut() {
        let layer_index = items
            .iter()
            .position(|item| item.is("layer"))
            .map(|index| index + 1)
            .unwrap_or(2);
        items.insert(layer_index, sexp::parse("(at 100 100)").expect("at"));
    }
    let version = pcb.version.as_deref().unwrap_or(FALLBACK_PCB_VERSION);
    let layers = pcb
        .layers
        .as_ref()
        .map(|layers| layers.to_string())
        .unwrap_or_else(|| FALLBACK_LAYERS.to_string());
    format!(
        "(kicad_pcb (version {version}) (generator \"kicad-pcb-library\") (general (thickness {thickness})) (paper \"A4\") {layers} (setup (pad_to_mask_clearance 0)) (net 0 \"\") {node})\n"
    )
}

/// Footprint reduced to only visible 3D model nodes.
///
/// The library view already renders the footprint itself in the Footprint cell.
/// Rendering only the model here lets parts that reuse the same 3D asset share a
/// single PNG/GLB render job instead of paying for the same STEP many times.
fn model_only_footprint(fp: &Sexp) -> Sexp {
    let mut items = vec![
        Sexp::atom("footprint"),
        Sexp::string("kicad-pcb-model"),
        sexp::parse("(layer \"F.Cu\")").expect("static layer"),
        sexp::parse("(attr smd)").expect("static attr"),
    ];
    items.extend(fp.children("model").cloned());
    Sexp::List(items)
}

/// Stable identity for cache sharing across different paths to identical model
/// bytes. The real render input keeps real paths so KiCad can open the model.
fn model_cache_identity(model_node: &Sexp) -> String {
    let mut identity = model_node.clone();
    identity.walk_mut(&mut |node| {
        if !node.is("model") {
            return;
        }
        let Some(path) = node.str_at(1) else {
            return;
        };
        let digest = file_digest(Path::new(&path)).unwrap_or_else(|| format!("missing:{path}"));
        if let Some(fields) = node.items_mut() {
            if fields.len() > 1 {
                fields[1] = Sexp::string(&format!("sha256:{digest}"));
            }
        }
    });
    identity.to_string()
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Bounds {
    fn new() -> Self {
        Self {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    fn add(&mut self, x: f64, y: f64) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    fn is_valid(&self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
            && self.max_x > self.min_x
            && self.max_y > self.min_y
    }

    fn max_extent(&self) -> f64 {
        (self.max_x - self.min_x).max(self.max_y - self.min_y)
    }
}

fn node_num(node: &Sexp, index: usize) -> Option<f64> {
    node.str_at(index)?.parse().ok()
}

fn add_xy_node(bounds: &mut Bounds, node: &Sexp) {
    if node.is("xy") {
        if let (Some(x), Some(y)) = (node_num(node, 1), node_num(node, 2)) {
            bounds.add(x, y);
        }
    }
}

fn add_pts(bounds: &mut Bounds, node: &Sexp) {
    for child in node.children("xy") {
        add_xy_node(bounds, child);
    }
}

fn footprint_bounds(fp: &Sexp) -> Option<Bounds> {
    let mut bounds = Bounds::new();
    for child in fp.items().iter().skip(2) {
        if child.is("pad") {
            let at = child.child("at");
            let size = child.child("size");
            if let (Some(x), Some(y), Some(w), Some(h)) = (
                at.and_then(|at| node_num(at, 1)),
                at.and_then(|at| node_num(at, 2)),
                size.and_then(|size| node_num(size, 1)),
                size.and_then(|size| node_num(size, 2)),
            ) {
                let half = w.max(h) / 2.0;
                bounds.add(x - half, y - half);
                bounds.add(x + half, y + half);
            }
        } else if matches!(
            child.head(),
            Some("fp_line" | "fp_arc" | "fp_rect" | "fp_curve")
        ) {
            for point in ["start", "mid", "end", "center"] {
                if let Some(node) = child.child(point) {
                    if let (Some(x), Some(y)) = (node_num(node, 1), node_num(node, 2)) {
                        bounds.add(x, y);
                    }
                }
            }
        } else if child.is("fp_circle") {
            if let (Some(center), Some(end)) = (child.child("center"), child.child("end")) {
                if let (Some(cx), Some(cy), Some(ex), Some(ey)) = (
                    node_num(center, 1),
                    node_num(center, 2),
                    node_num(end, 1),
                    node_num(end, 2),
                ) {
                    let radius = ((ex - cx).powi(2) + (ey - cy).powi(2)).sqrt();
                    bounds.add(cx - radius, cy - radius);
                    bounds.add(cx + radius, cy + radius);
                }
            }
        } else if child.is("fp_poly") {
            if let Some(pts) = child.child("pts") {
                add_pts(&mut bounds, pts);
            }
        }
    }
    bounds.is_valid().then_some(bounds)
}

fn model_render_params(fp: &Sexp) -> Vec<String> {
    let zoom = footprint_bounds(fp)
        .map(|bounds| (2.8 / bounds.max_extent()).clamp(0.35, 0.95))
        .unwrap_or(0.45);
    vec!["--zoom".to_string(), format!("{zoom:.3}")]
}

/// Rewrite model paths to resolved absolute files, dropping missing models.
/// Returns the model refs and the files that exist.
fn resolve_models(fp: &mut Sexp, resolver: &Resolver) -> (Vec<ModelRef>, Vec<PathBuf>) {
    let mut refs = Vec::new();
    let mut files = Vec::new();
    let Some(items) = fp.items_mut() else {
        return (refs, files);
    };
    items.retain_mut(|item| {
        if !item.is("model") {
            return true;
        }
        let Some(path) = item.str_at(1) else {
            return false;
        };
        let hidden = item
            .child("hide")
            .is_some_and(|hide| hide.str_at(1).as_deref() != Some("no"))
            || item.items().iter().any(|atom| atom == &Sexp::atom("hide"));
        let resolved = resolver.path(&path);
        let found = resolved.is_file();
        refs.push(ModelRef {
            path: path.clone(),
            resolved: Some(resolved.display().to_string()),
            found,
        });
        if !found || hidden {
            return false;
        }
        if let Some(fields) = item.items_mut() {
            fields[1] = Sexp::string(&resolved.display().to_string());
        }
        files.push(resolved);
        true
    });
    (refs, files)
}

/// Drift comparison form of a footprint: pad, graphic and model geometry only.
///
/// Whitelisting keeps the comparison robust to cosmetic/serializer noise
/// (uuids, default-valued pad attributes, stroke styles, reference text) that
/// differs between pcbnew board saves and library files.
fn normalize_footprint(fp: &Sexp) -> String {
    const PAD_KEEP: &[&str] = &[
        "at",
        "size",
        "drill",
        "layers",
        "roundrect_rratio",
        "chamfer_ratio",
        "chamfer",
        "rect_delta",
        "primitives",
        "options",
    ];
    const GRAPHICS: &[&str] = &[
        "fp_line",
        "fp_arc",
        "fp_circle",
        "fp_rect",
        "fp_poly",
        "fp_curve",
    ];
    const GRAPHIC_KEEP: &[&str] = &["start", "end", "mid", "center", "pts", "layer"];
    let mut node = fp.clone();
    unplace(&mut node);
    let mut items = Vec::new();
    for child in node.items().iter().skip(2) {
        if child.is("pad") {
            let mut pad = child.clone();
            pad.retain_children(|item| {
                matches!(item, Sexp::Atom(_)) || PAD_KEEP.iter().any(|head| item.is(head))
            });
            // Layer order within a pad is not significant.
            if let Some(items) = pad.items_mut() {
                for item in items.iter_mut().filter(|item| item.is("layers")) {
                    if let Some(layers) = item.items_mut() {
                        let mut names = layers.split_off(1);
                        names.sort_by_key(|layer| match layer {
                            Sexp::Atom(raw) => sexp::unquote(raw),
                            other => other.to_string(),
                        });
                        layers.extend(names);
                    }
                }
            }
            items.push(canonical(&pad));
        } else if GRAPHICS.iter().any(|head| child.is(head)) {
            let width = child
                .child("stroke")
                .and_then(|stroke| stroke.child("width"))
                .or_else(|| child.child("width"))
                .and_then(|width| width.str_at(1))
                .and_then(|width| sexp::normalize_number(&width))
                .unwrap_or_default();
            let filled = child.child("fill").is_some_and(|fill| {
                let value = fill
                    .child("type")
                    .and_then(|kind| kind.str_at(1))
                    .or_else(|| fill.str_at(1))
                    .unwrap_or_default();
                matches!(value.as_str(), "yes" | "solid")
            });
            let mut graphic = child.clone();
            graphic.retain_children(|item| GRAPHIC_KEEP.iter().any(|head| item.is(head)));
            items.push(format!("{} w={width} fill={filled}", canonical(&graphic)));
        } else if child.is("model") {
            let mut model = child.clone();
            model.retain_children(|item| {
                matches!(item, Sexp::Atom(_))
                    || ["offset", "scale", "rotate"]
                        .iter()
                        .any(|head| item.is(head))
            });
            items.push(canonical(&model));
        }
    }
    items.sort();
    items.join("\n")
}

fn canonical(node: &Sexp) -> String {
    match node {
        Sexp::Atom(raw) => sexp::normalize_number(raw).unwrap_or_else(|| sexp::unquote(raw)),
        Sexp::List(items) => format!(
            "({})",
            items.iter().map(canonical).collect::<Vec<_>>().join(" ")
        ),
    }
}

/// Drift comparison form of a symbol definition.
fn normalize_symbol(symbol: &Sexp) -> String {
    let mut node = symbol.clone();
    node.walk_mut(&mut |item| {
        if item.is("symbol") {
            rename(item, "symbol");
        }
    });
    strip_heads(
        &mut node,
        &["uuid", "embedded_fonts", "generator", "generator_version"],
    );
    // Property placement and styling is cosmetic; compare names/values only.
    node.retain_children(|child| !child.is("property"));
    let mut props = symbol
        .properties()
        .into_iter()
        .filter(|(name, _)| name != "Reference" && name != "Value")
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>();
    props.sort();
    format!("{}|{}", canonical_children(&node, true), props.join(";"))
}

fn symbol_group_identity(sch: &SchData, instance: Option<&SchInstance>) -> String {
    let Some(instance) = instance else {
        return String::new();
    };
    extract_embedded_symbol(sch, &instance.cache_name)
        .map(|symbol| format!("shape:{}", normalize_symbol(&symbol)))
        .unwrap_or_else(|| format!("lib:{}", instance.lib_id))
}

fn exact_part_identity(fields: &[(String, String)]) -> Option<String> {
    let mut exact = Vec::new();
    let mut manufacturer = Vec::new();
    for (name, value) in fields {
        if !is_meaningful(value) {
            continue;
        }
        match field_kind(name) {
            Some("lcsc" | "mpn") => exact.push(format!(
                "{}={}",
                name.chars()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase(),
                value.trim()
            )),
            Some("manufacturer") => manufacturer.push(format!(
                "{}={}",
                name.chars()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase(),
                value.trim()
            )),
            _ => {}
        }
    }
    if exact.is_empty() {
        return None;
    }
    exact.extend(manufacturer);
    exact.sort();
    exact.dedup();
    Some(exact.join("|"))
}

fn part_group_identity(fields: &[(String, String)]) -> String {
    if let Some(exact) = exact_part_identity(fields) {
        return exact;
    }
    let mut values = fields
        .iter()
        .filter_map(|(name, value)| {
            if !is_meaningful(value) {
                return None;
            }
            if name == "Value" || matches!(field_kind(name), Some("lcsc" | "mpn" | "manufacturer"))
            {
                Some(format!(
                    "{}={}",
                    name.chars()
                        .filter(|ch| ch.is_ascii_alphanumeric())
                        .collect::<String>()
                        .to_ascii_lowercase(),
                    value.trim()
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values.join("|")
}

fn is_test_point_part(reference: &str, symbol: Option<&str>, fpid: Option<&str>) -> bool {
    let has_tp_ref = reference
        .strip_prefix("TP")
        .is_some_and(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()));
    let names_test_point = symbol
        .into_iter()
        .chain(fpid)
        .any(|name| name.to_ascii_lowercase().contains("testpoint"));
    has_tp_ref || names_test_point
}

fn grouped_part_identity(
    reference: &str,
    symbol: Option<&str>,
    fpid: Option<&str>,
    fields: &[(String, String)],
) -> String {
    if is_test_point_part(reference, symbol, fpid) {
        String::new()
    } else {
        part_group_identity(fields)
    }
}

fn physical_part_symbol_identity(
    sch: &SchData,
    instance: Option<&SchInstance>,
    fpid: Option<&str>,
    fields: &[(String, String)],
) -> String {
    if fpid.is_some_and(is_meaningful) && exact_part_identity(fields).is_some() {
        String::new()
    } else {
        symbol_group_identity(sch, instance)
    }
}

/// Serialize with numbers normalized, strings unquoted, the item name's
/// library nickname removed, and (optionally) top-level children sorted.
fn canonical_children(node: &Sexp, sort: bool) -> String {
    let canon = canonical;
    let items = node.items();
    let head = items.first().map(canon).unwrap_or_default();
    let name = node
        .str_at(1)
        .map(|name| split_lib_id(&name).1.to_string())
        .unwrap_or_default();
    let mut children = items.iter().skip(2).map(canon).collect::<Vec<_>>();
    if sort {
        children.sort();
    }
    format!("({head} {name} {})", children.join(" "))
}

/// Embedded (schematic cache) symbol renamed to its bare library name.
fn extract_embedded_symbol(sch: &SchData, cache_name: &str) -> Option<Sexp> {
    let mut node = sch.lib_symbols.get(cache_name)?.clone();
    let bare = split_lib_id(cache_name).1.to_string();
    rename(&mut node, &bare);
    Some(node)
}

/// Many generated libraries stack Reference and Value at the same spot; nudge
/// Value down one grid step so the thumbnail stays legible.
fn declutter_fields(symbol: &mut Sexp) {
    let position = |node: &Sexp, name: &str| {
        node.children("property")
            .find(|prop| prop.str_at(1).as_deref() == Some(name))
            .and_then(|prop| prop.child("at").cloned())
    };
    let (Some(reference), Some(value)) = (position(symbol, "Reference"), position(symbol, "Value"))
    else {
        return;
    };
    if reference != value {
        return;
    }
    let Some(items) = symbol.items_mut() else {
        return;
    };
    for item in items.iter_mut() {
        if !(item.is("property") && item.str_at(1).as_deref() == Some("Value")) {
            continue;
        }
        let Some(fields) = item.items_mut() else {
            continue;
        };
        for field in fields.iter_mut().filter(|field| field.is("at")) {
            let y = field
                .str_at(2)
                .and_then(|y| y.parse::<f64>().ok())
                .unwrap_or(0.0);
            if let Some(at) = field.items_mut() {
                if at.len() > 2 {
                    at[2] = Sexp::atom(
                        sexp::normalize_number(&format!("{:.4}", y - 2.54)).unwrap_or_default(),
                    );
                }
            }
        }
    }
}

fn symbol_lib_text(symbols: &[Sexp], version: &str) -> String {
    let body = symbols
        .iter()
        .map(|symbol| {
            let mut symbol = symbol.clone();
            declutter_fields(&mut symbol);
            let bare = symbol
                .str_at(1)
                .map(|name| split_lib_id(&name).1.to_string())
                .unwrap_or_default();
            rename(&mut symbol, &bare);
            symbol.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("(kicad_symbol_lib (version {version}) (generator \"kicad-pcb-library\") {body})\n")
}

/// Minimal schematic placing one symbol, for the KiCanvas viewer.
fn symbol_sch_text(symbol: &Sexp, lib_id: &str, reference: &str, version: &str) -> String {
    let mut cached = symbol.clone();
    rename(&mut cached, lib_id);
    let mut props = String::new();
    for (index, (name, value)) in symbol.properties().into_iter().enumerate() {
        let value = if name == "Reference" {
            reference.to_string()
        } else {
            value
        };
        let hidden = if name == "Reference" || name == "Value" {
            ""
        } else {
            " (hide yes)"
        };
        props.push_str(&format!(
            " (property {} {} (at 127 {} 0) (effects (font (size 1.27 1.27)){hidden}))",
            sexp::quote(&name),
            sexp::quote(&value),
            80 + index * 3,
        ));
    }
    format!(
        "(kicad_sch (version {version}) (generator \"kicad-pcb-library\") (uuid \"00000000-0000-0000-0000-000000000001\") (paper \"A4\") (lib_symbols {cached}) (symbol (lib_id {}) (at 127 88.9 0) (unit 1) (in_bom yes) (on_board yes) (uuid \"00000000-0000-0000-0000-000000000002\"){props}))\n",
        sexp::quote(lib_id)
    )
}

// ---------------------------------------------------------------------------
// Inventory construction

#[derive(Debug, Default)]
struct Group {
    symbol: Option<String>,
    cache_name: Option<String>,
    footprint: Option<String>,
    models: Vec<String>,
    refs: BTreeSet<String>,
    values: BTreeSet<String>,
    fields: Vec<(String, String)>,
    board: Option<PcbFootprint>,
    power: bool,
}

fn field_kind(name: &str) -> Option<&'static str> {
    let key = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if key.starts_with("lcsc") || key.starts_with("jlc") {
        return Some("lcsc");
    }
    match key.as_str() {
        "mpn"
        | "mfrpart"
        | "mfrpn"
        | "mfrpartnumber"
        | "manufacturerpartnumber"
        | "manufacturerpn"
        | "manufacturerpart"
        | "partnumber"
        | "mfgpart"
        | "mfgpn" => Some("mpn"),
        "manufacturer" | "mfr" | "mfg" | "manufacturername" => Some("manufacturer"),
        "datasheet" => Some("datasheet"),
        "description" => Some("description"),
        _ => None,
    }
}

fn pick_field(fields: &[(String, String)], kind: &str) -> Option<String> {
    fields
        .iter()
        .filter(|(name, value)| field_kind(name) == Some(kind) && is_meaningful(value))
        .map(|(_, value)| value.trim().to_string())
        .next()
}

fn is_meaningful(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "~" && value != "-"
}

fn badge(kind: &str, level: &str, label: &str, detail: Option<String>) -> LibraryBadge {
    LibraryBadge {
        kind: kind.to_string(),
        level: level.to_string(),
        label: label.to_string(),
        detail,
    }
}

fn short_hash(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_string()
}

struct InvCtx {
    resolver: Resolver,
    libs: Libraries,
    sch: SchData,
    pcb: PcbData,
}

fn build_inventory(repo_root: &Path, project: &ProjectContext) -> Result<Inventory> {
    let resolver = Resolver::new(&project.root);
    let (libs, mut warnings) = Libraries::load(repo_root, project, &resolver);
    let sch = match crate::pick_project_file(project, "kicad_sch")? {
        Some(path) => load_schematic(&path).unwrap_or_else(|err| {
            warnings.push(format!("schematic: {err:#}"));
            SchData::default()
        }),
        None => SchData::default(),
    };
    let pcb = match crate::pick_project_file(project, "kicad_pcb")? {
        Some(path) => load_pcb(&path).unwrap_or_else(|err| {
            warnings.push(format!("pcb: {err:#}"));
            PcbData::default()
        }),
        None => PcbData::default(),
    };
    let ctx = InvCtx {
        resolver,
        libs,
        sch,
        pcb,
    };

    // Join schematic instances with board footprints by reference.
    let mut by_ref: BTreeMap<String, (Option<&SchInstance>, Option<&PcbFootprint>)> =
        BTreeMap::new();
    let mut power_symbols: BTreeMap<String, &SchInstance> = BTreeMap::new();
    for instance in &ctx.sch.instances {
        if instance.power {
            power_symbols
                .entry(instance.lib_id.clone())
                .or_insert(instance);
            continue;
        }
        for reference in &instance.references {
            by_ref.entry(reference.clone()).or_default().0 = Some(instance);
        }
    }
    for footprint in &ctx.pcb.footprints {
        if footprint.reference.is_empty() || footprint.reference == "REF**" {
            continue;
        }
        by_ref
            .entry(footprint.reference.clone())
            .or_default()
            .1
            .get_or_insert(footprint);
    }

    let mut groups: BTreeMap<(String, String, String, String), Group> = BTreeMap::new();
    for (reference, (instance, footprint)) in &by_ref {
        let symbol = instance.map(|instance| instance.lib_id.clone());
        let fpid = footprint.map(|fp| fp.fpid.clone()).or_else(|| {
            instance
                .and_then(|instance| instance.fields.iter().find(|(name, _)| name == "Footprint"))
                .map(|(_, value)| value.clone())
                .filter(|value| is_meaningful(value))
        });
        let models = footprint
            .map(|fp| {
                let mut models = fp
                    .node
                    .children("model")
                    .filter_map(|model| model.str_at(1))
                    .collect::<Vec<_>>();
                models.sort();
                models.dedup();
                models
            })
            .unwrap_or_default();
        let mut part_fields = Vec::new();
        if let Some(instance) = instance {
            part_fields.extend(instance.fields.iter().cloned());
        }
        if let Some(footprint) = footprint {
            part_fields.extend(footprint.fields.iter().cloned());
        }
        let part_identity =
            grouped_part_identity(reference, symbol.as_deref(), fpid.as_deref(), &part_fields);
        let key = (
            physical_part_symbol_identity(&ctx.sch, *instance, fpid.as_deref(), &part_fields),
            fpid.clone().unwrap_or_default(),
            models.join("|"),
            part_identity,
        );
        let group = groups.entry(key).or_default();
        if let Some(symbol) = symbol {
            group.symbol.get_or_insert(symbol);
        }
        group.footprint = fpid;
        group.models = models;
        group.refs.insert(reference.clone());
        if let Some(instance) = instance {
            group.cache_name.get_or_insert(instance.cache_name.clone());
            if let Some((_, value)) = instance.fields.iter().find(|(name, _)| name == "Value") {
                group.values.insert(value.clone());
            }
            group.fields.extend(instance.fields.iter().cloned());
        } else if let Some(fp) = footprint {
            if let Some((_, value)) = fp.fields.iter().find(|(name, _)| name == "Value") {
                group.values.insert(value.clone());
            }
        }
        if let Some(fp) = footprint {
            group.fields.extend(fp.fields.iter().cloned());
            if group.board.is_none()
                || (group.board.as_ref().is_some_and(|b| b.bottom) && !fp.bottom)
            {
                group.board = Some((*fp).clone());
            }
        }
    }
    for (lib_id, instance) in power_symbols {
        groups
            .entry((lib_id.clone(), String::new(), String::new(), String::new()))
            .or_insert_with(|| Group {
                symbol: Some(lib_id.clone()),
                cache_name: Some(instance.cache_name.clone()),
                values: instance
                    .fields
                    .iter()
                    .filter(|(name, _)| name == "Value")
                    .map(|(_, value)| value.clone())
                    .collect(),
                fields: instance.fields.clone(),
                power: true,
                ..Group::default()
            });
    }

    let mut parts = groups
        .into_values()
        .map(|group| part_from_group(&ctx, group))
        .collect::<Vec<_>>();
    parts.sort_by(|a, b| {
        let key = |draft: &PartDraft| {
            (
                draft.part.refs.is_empty(),
                draft
                    .part
                    .refs
                    .first()
                    .map(|reference| crate::natural_ref_key(reference)),
                draft.part.title.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    let used_symbols = parts
        .iter()
        .filter_map(|draft| draft.part.symbol.clone())
        .collect::<HashSet<_>>();
    let used_footprints = parts
        .iter()
        .filter_map(|draft| draft.part.footprint.clone())
        .collect::<HashSet<_>>();
    let unused = unused_items(&ctx, &used_symbols, &used_footprints);

    Ok(Inventory {
        project: project.id.clone(),
        parts,
        unused,
        libraries: ctx.libs.sources(),
        warnings,
    })
}

fn part_from_group(ctx: &InvCtx, group: Group) -> PartDraft {
    let mut badges = Vec::new();
    let first_ref = group
        .refs
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| "X1".to_string());

    // Symbol: prefer the schematic's embedded copy (what the design uses).
    let symbol = match (&group.symbol, &group.cache_name) {
        (Some(lib_id), cache_name) => {
            let embedded = cache_name
                .as_deref()
                .and_then(|name| extract_embedded_symbol(&ctx.sch, name));
            let library = library_symbol(&ctx.libs, lib_id);
            if let (Some(embedded), Some((_, chain))) = (&embedded, library.found()) {
                if chain.len() == 1 && normalize_symbol(embedded) != normalize_symbol(&chain[0]) {
                    badges.push(badge(
                        "symbol-drift",
                        "warning",
                        "symbol drift",
                        Some("schematic copy differs from the library symbol".to_string()),
                    ));
                }
            }
            match &library {
                LibraryLookup::Missing(path) => badges.push(badge(
                    "symbol-not-in-library",
                    "warning",
                    "symbol not in library",
                    Some(format!("{lib_id} missing from {}", path.display())),
                )),
                LibraryLookup::NoLibrary if embedded.is_none() => badges.push(badge(
                    "symbol-library-missing",
                    "warning",
                    "symbol library not found",
                    Some(lib_id.clone()),
                )),
                _ => {}
            }
            let version = ctx.sch.version.as_deref().unwrap_or(FALLBACK_SYM_VERSION);
            match (embedded, library) {
                (Some(node), _) => symbol_draft(
                    vec![node],
                    lib_id,
                    &first_ref,
                    version,
                    "schematic",
                    version,
                ),
                (None, LibraryLookup::Found((lib_version, chain))) => {
                    symbol_draft(chain, lib_id, &first_ref, &lib_version, "library", version)
                }
                _ => ThumbDraft::none("symbol source not found"),
            }
        }
        (None, _) => ThumbDraft::none("no schematic symbol"),
    };

    // Footprint and 3D: prefer the board copy unless it is flipped to the
    // bottom and a library copy exists.
    let (footprint, model, models) = match &group.footprint {
        Some(fpid) => {
            let library = library_footprint(&ctx.libs, fpid);
            let name = split_lib_id(fpid).1.to_string();
            if let (Some(board), Some((_, lib_fp))) = (&group.board, library.found()) {
                if board.bottom {
                    badges.push(badge(
                        "bottom-side",
                        "info",
                        "bottom side",
                        Some("drift check skipped for flipped footprint".to_string()),
                    ));
                } else if normalize_footprint(&board.node) != normalize_footprint(lib_fp) {
                    badges.push(badge(
                        "footprint-drift",
                        "warning",
                        "footprint drift",
                        Some("board copy differs from the library footprint".to_string()),
                    ));
                }
            }
            match &library {
                LibraryLookup::Missing(path) => badges.push(badge(
                    "footprint-not-in-library",
                    "warning",
                    "footprint not in library",
                    Some(format!("{fpid} missing from {}", path.display())),
                )),
                LibraryLookup::NoLibrary if group.board.is_none() => badges.push(badge(
                    "footprint-library-missing",
                    "warning",
                    "footprint library not found",
                    Some(fpid.clone()),
                )),
                _ => {}
            }
            let source = match (&group.board, library.found()) {
                (Some(board), Some((_, lib_fp))) if board.bottom => {
                    Some((lib_fp.clone(), "library"))
                }
                (Some(board), _) => Some((canonical_board_footprint(&board.node, &name), "board")),
                (None, Some((_, lib_fp))) => Some((lib_fp.clone(), "library")),
                (None, None) => None,
            };
            match source {
                Some((node, origin)) => footprint_drafts(ctx, &node, fpid, origin, &mut badges),
                None => (
                    ThumbDraft::none("footprint not found"),
                    ThumbDraft::none("footprint not found"),
                    Vec::new(),
                ),
            }
        }
        None => (
            ThumbDraft::none("no footprint"),
            ThumbDraft::none("no footprint"),
            Vec::new(),
        ),
    };
    if group.power {
        badges.push(badge("power", "info", "power symbol", None));
    }

    let fields = &group.fields;
    let title = group
        .footprint
        .as_deref()
        .or(group.symbol.as_deref())
        .unwrap_or("unknown")
        .to_string();
    let id = short_hash(&[
        group.symbol.as_deref().unwrap_or_default(),
        group.footprint.as_deref().unwrap_or_default(),
        &group.models.join("|"),
    ]);
    PartDraft {
        part: LibraryPart {
            id,
            title,
            symbol: group.symbol.clone(),
            footprint: group.footprint.clone(),
            refs: {
                let mut refs = group.refs.into_iter().collect::<Vec<_>>();
                refs.sort_by_key(|reference| crate::natural_ref_key(reference));
                refs
            },
            values: group
                .values
                .into_iter()
                .filter(|v| is_meaningful(v))
                .collect(),
            lcsc: pick_field(fields, "lcsc"),
            mpn: pick_field(fields, "mpn"),
            manufacturer: pick_field(fields, "manufacturer"),
            datasheet: pick_field(fields, "datasheet"),
            description: pick_field(fields, "description"),
            models,
            badges,
            thumbs: PartThumbs::default(),
        },
        symbol,
        footprint,
        model,
    }
}

fn symbol_draft(
    chain: Vec<Sexp>,
    lib_id: &str,
    reference: &str,
    lib_version: &str,
    origin: &str,
    sch_version: &str,
) -> ThumbDraft {
    let name = split_lib_id(lib_id).1.to_string();
    let viewer_symbol = chain.last().cloned();
    ThumbDraft {
        spec: Some(RenderSpec {
            kind: RenderKind::Symbol,
            item: name,
            input: symbol_lib_text(&chain, lib_version),
            cache_identity: None,
            params: Vec::new(),
            model_files: Vec::new(),
        }),
        placeholder: None,
        source: Some(origin.to_string()),
        // Only single-level symbols can be embedded without flattening.
        viewer: viewer_symbol.filter(|_| chain.len() == 1).map(|symbol| {
            (
                "kicad_sch",
                symbol_sch_text(&symbol, lib_id, reference, sch_version),
            )
        }),
        glb: None,
    }
}

fn footprint_drafts(
    ctx: &InvCtx,
    node: &Sexp,
    fpid: &str,
    origin: &str,
    badges: &mut Vec<LibraryBadge>,
) -> (ThumbDraft, ThumbDraft, Vec<ModelRef>) {
    let version = ctx.pcb.version.as_deref().unwrap_or(FALLBACK_PCB_VERSION);
    let viewer_pcb = footprint_pcb_text(node, fpid, "REF**", &ctx.pcb, "1.6");
    let footprint = ThumbDraft {
        spec: Some(RenderSpec {
            kind: RenderKind::Footprint,
            item: "item".to_string(),
            input: footprint_mod_text(node, "item", version),
            cache_identity: None,
            params: Vec::new(),
            model_files: Vec::new(),
        }),
        placeholder: None,
        source: Some(origin.to_string()),
        viewer: Some(("kicad_pcb", viewer_pcb)),
        glb: None,
    };

    let mut with_models = node.clone();
    let (models, files) = resolve_models(&mut with_models, &ctx.resolver);
    let missing = models.iter().filter(|model| !model.found).count();
    let model = if models.is_empty() {
        badges.push(badge("no-3d-model", "warning", "no 3D model", None));
        ThumbDraft::none("no 3D model linked from footprint")
    } else if files.is_empty() {
        badges.push(badge(
            "model-missing",
            "error",
            "model file not found",
            Some(
                models
                    .iter()
                    .filter(|model| !model.found)
                    .map(|model| model.path.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ));
        if missing == 0 {
            ThumbDraft::none("3D model hidden")
        } else {
            ThumbDraft::none("3D model file not found")
        }
    } else {
        if missing > 0 {
            badges.push(badge(
                "model-missing",
                "error",
                "model file not found",
                Some(
                    models
                        .iter()
                        .filter(|model| !model.found)
                        .map(|model| model.path.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ));
        }
        let model_node = model_only_footprint(&with_models);
        let cache_identity = model_cache_identity(&model_node);
        let model_pcb = footprint_pcb_text(
            &with_models,
            "kicad-pcb-model",
            "",
            &ctx.pcb,
            THUMB_BOARD_THICKNESS,
        );
        let glb_pcb = footprint_pcb_text(&model_node, "kicad-pcb-model", "", &ctx.pcb, "1.6");
        let render_params = model_render_params(&with_models);
        ThumbDraft {
            spec: Some(RenderSpec {
                kind: RenderKind::Model,
                item: String::new(),
                input: model_pcb,
                cache_identity: Some(cache_identity.clone()),
                params: render_params,
                model_files: files.clone(),
            }),
            placeholder: None,
            source: Some(origin.to_string()),
            viewer: None,
            glb: Some(RenderSpec {
                kind: RenderKind::Glb,
                item: String::new(),
                input: glb_pcb,
                cache_identity: Some(cache_identity),
                params: Vec::new(),
                model_files: files,
            }),
        }
    };
    (footprint, model, models)
}

fn unused_items(
    ctx: &InvCtx,
    used_symbols: &HashSet<String>,
    used_footprints: &HashSet<String>,
) -> Vec<PartDraft> {
    let mut drafts = Vec::new();
    for (kind, nick) in &ctx.libs.order {
        match kind {
            LibKind::Symbol => {
                let Some(entry) = ctx.libs.symbols.get(nick).filter(|entry| entry.local) else {
                    continue;
                };
                let Some(lib) = load_symbol_lib(&entry.path) else {
                    continue;
                };
                for (name, _) in &lib.symbols {
                    let lib_id = format!("{nick}:{name}");
                    if used_symbols.contains(&lib_id) {
                        continue;
                    }
                    let Some(chain) = lib.with_parents(name) else {
                        continue;
                    };
                    let node = chain.last().cloned().expect("non-empty chain");
                    let fields = node.properties();
                    let version = ctx.sch.version.as_deref().unwrap_or(FALLBACK_SYM_VERSION);
                    let reference = node
                        .property("Reference")
                        .map(|prefix| format!("{}?", prefix.trim_end_matches('?')))
                        .unwrap_or_else(|| "U?".to_string());
                    drafts.push(PartDraft {
                        part: LibraryPart {
                            id: short_hash(&["unused-symbol", &lib_id]),
                            title: lib_id.clone(),
                            symbol: Some(lib_id.clone()),
                            footprint: node
                                .property("Footprint")
                                .filter(|value| is_meaningful(value)),
                            values: node.property("Value").into_iter().collect(),
                            lcsc: pick_field(&fields, "lcsc"),
                            mpn: pick_field(&fields, "mpn"),
                            manufacturer: pick_field(&fields, "manufacturer"),
                            datasheet: pick_field(&fields, "datasheet"),
                            description: pick_field(&fields, "description"),
                            badges: vec![badge("unused", "info", "unused symbol", None)],
                            ..LibraryPart::default()
                        },
                        symbol: symbol_draft(
                            chain,
                            &lib_id,
                            &reference,
                            &lib.version,
                            "library",
                            version,
                        ),
                        footprint: ThumbDraft::none("symbol only"),
                        model: ThumbDraft::none("symbol only"),
                    });
                }
            }
            LibKind::Footprint => {
                let Some(entry) = ctx.libs.footprints.get(nick).filter(|entry| entry.local) else {
                    continue;
                };
                for name in footprint_names(&entry.path) {
                    let fpid = format!("{nick}:{name}");
                    if used_footprints.contains(&fpid) {
                        continue;
                    }
                    let LibraryLookup::Found((_, node)) = library_footprint(&ctx.libs, &fpid)
                    else {
                        continue;
                    };
                    let mut badges = vec![badge("unused", "info", "unused footprint", None)];
                    let (footprint, model, models) =
                        footprint_drafts(ctx, &node, &fpid, "library", &mut badges);
                    drafts.push(PartDraft {
                        part: LibraryPart {
                            id: short_hash(&["unused-footprint", &fpid]),
                            title: fpid.clone(),
                            footprint: Some(fpid.clone()),
                            description: node.child("descr").and_then(|d| d.str_at(1)),
                            models,
                            badges,
                            ..LibraryPart::default()
                        },
                        symbol: ThumbDraft::none("footprint only"),
                        footprint,
                        model,
                    });
                }
            }
        }
    }
    drafts
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD_FP: &str = r#"(footprint "Lib:R_0402"
        (layer "F.Cu")
        (uuid "11111111-1111-1111-1111-111111111111")
        (at 103 123.5 90)
        (descr "Resistor 0402")
        (property "Reference" "R7" (at 0 -1.2 90) (layer "F.SilkS") (uuid "22222222-2222-2222-2222-222222222222") (effects (font (size 1 1) (thickness 0.15))))
        (property "Value" "10k" (at 0 1.2 90) (layer "F.Fab") (uuid "33333333-3333-3333-3333-333333333333") (effects (font (size 1 1) (thickness 0.15))))
        (property "LCSC" "C25744" (at 0 0 0) (layer "F.Fab") (hide yes) (uuid "44444444-4444-4444-4444-444444444444") (effects (font (size 1 1))))
        (path "/aaaa/bbbb")
        (sheetname "/")
        (sheetfile "board.kicad_sch")
        (attr smd)
        (fp_line (start -0.5 -0.25) (end 0.5 -0.25) (stroke (width 0.1) (type solid)) (layer "F.Fab") (uuid "55555555-5555-5555-5555-555555555555"))
        (pad "1" smd roundrect (at -0.48 0 90) (size 0.56 0.62) (layers "F.Cu" "F.Mask" "F.Paste") (roundrect_rratio 0.25) (net 3 "VCC") (pinfunction "~") (pintype "passive") (uuid "66666666-6666-6666-6666-666666666666"))
        (pad "2" smd roundrect (at 0.48 0 90) (size 0.56 0.62) (layers "F.Cu" "F.Mask" "F.Paste") (roundrect_rratio 0.25) (net 1 "GND") (uuid "77777777-7777-7777-7777-777777777777"))
        (model "${KIPRJMOD}/3dmodels/R_0402.step" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0)))
    )"#;

    const LIB_FP: &str = r#"(footprint "R_0402"
        (version 20240108)
        (generator "pcbnew")
        (layer "F.Cu")
        (descr "Resistor 0402")
        (property "Reference" "REF**" (at 0 -1.2 0) (layer "F.SilkS") (uuid "a") (effects (font (size 1 1) (thickness 0.15))))
        (property "Value" "R_0402" (at 0 1.2 0) (layer "F.Fab") (uuid "b") (effects (font (size 1 1) (thickness 0.15))))
        (attr smd)
        (fp_line (start -0.5 -0.25) (end 0.50000 -0.25) (stroke (width 0.1) (type solid)) (layer "F.Fab") (uuid "c"))
        (pad "1" smd roundrect (at -0.48 0) (size 0.56 0.62) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25) (uuid "d"))
        (pad "2" smd roundrect (at 0.48 0) (size 0.56 0.62) (layers "F.Cu" "F.Mask" "F.Paste") (roundrect_rratio 0.25) (uuid "e"))
        (model "${KIPRJMOD}/3dmodels/R_0402.step" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0)))
    )"#;

    #[test]
    fn board_footprint_matches_library_after_normalization() {
        let board = sexp::parse(BOARD_FP).unwrap();
        let lib = sexp::parse(LIB_FP).unwrap();
        assert_eq!(normalize_footprint(&board), normalize_footprint(&lib));
    }

    #[test]
    fn footprint_drift_is_detected() {
        let board = sexp::parse(&BOARD_FP.replace("(size 0.56 0.62) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\") (roundrect_rratio 0.25) (net 1", "(size 0.60 0.62) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\") (roundrect_rratio 0.25) (net 1")).unwrap();
        let lib = sexp::parse(LIB_FP).unwrap();
        assert_ne!(normalize_footprint(&board), normalize_footprint(&lib));
    }

    #[test]
    fn moving_a_footprint_keeps_its_render_input() {
        let a = sexp::parse(BOARD_FP).unwrap();
        let b = sexp::parse(
            &BOARD_FP
                .replace("(at 103 123.5 90)", "(at 55 20 90)")
                .replace("\"R7\"", "\"R9\"")
                .replace("11111111-1111", "99999999-1111"),
        )
        .unwrap();
        let text = |node: &Sexp| {
            footprint_mod_text(&canonical_board_footprint(node, "item"), "item", "20260206")
        };
        assert_eq!(text(&a), text(&b));
        let rendered = text(&a);
        assert!(rendered.starts_with("(footprint \"item\" (version 20260206)"));
        assert!(!rendered.contains("(net "));
        assert!(!rendered.contains("uuid"));
        assert!(rendered.contains("(property \"Reference\" \"\""));
        // Pad angles are made relative to the footprint again.
        assert!(rendered.contains("(pad \"1\" smd roundrect (at -0.48 0) "));
        sexp::parse(&rendered).unwrap();
    }

    #[test]
    fn extracts_embedded_symbol_and_detects_drift() {
        let sch_text = r#"(kicad_sch (version 20250114) (generator "eeschema")
            (lib_symbols
              (symbol "Lib:C" (pin_names (offset 1.016)) (in_bom yes) (on_board yes)
                (property "Reference" "C" (at 0 0 0) (effects (font (size 1.27 1.27))))
                (property "Value" "C" (at 0 0 0) (effects (font (size 1.27 1.27))))
                (symbol "C_0_1" (polyline (pts (xy -2.032 -0.762) (xy 2.032 -0.762)) (stroke (width 0.508) (type default)) (fill (type none))))
                (symbol "C_1_1" (pin passive line (at 0 3.81 270) (length 2.794) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))))
            (symbol (lib_id "Lib:C") (at 10 10 0) (unit 1)
              (property "Reference" "C4" (at 0 0 0))
              (property "Value" "100n" (at 0 0 0))
              (property "LCSC Part" "C1525" (at 0 0 0))
              (instances (project "p" (path "/x" (reference "C4") (unit 1)) (path "/y" (reference "C5") (unit 1))))))"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.kicad_sch");
        std::fs::write(&path, sch_text).unwrap();
        let sch = load_schematic(&path).unwrap();
        assert_eq!(sch.instances.len(), 1);
        assert_eq!(sch.instances[0].references, vec!["C4", "C5"]);
        assert_eq!(
            pick_field(&sch.instances[0].fields, "lcsc").as_deref(),
            Some("C1525")
        );

        let embedded = extract_embedded_symbol(&sch, "Lib:C").unwrap();
        assert_eq!(embedded.str_at(1).as_deref(), Some("C"));
        let lib_text = symbol_lib_text(std::slice::from_ref(&embedded), "20241209");
        assert!(lib_text.starts_with("(kicad_symbol_lib (version 20241209)"));
        assert!(lib_text.contains("(symbol \"C\" "));
        assert!(lib_text.contains("(symbol \"C_0_1\" "));
        let reparsed = sexp::parse(&lib_text).unwrap();
        let library_copy = reparsed.child("symbol").unwrap().clone();
        assert_eq!(normalize_symbol(&embedded), normalize_symbol(&library_copy));

        let drifted = sexp::parse(&library_copy.to_string().replace("2.794", "3.81")).unwrap();
        assert_ne!(normalize_symbol(&embedded), normalize_symbol(&drifted));

        let viewer = symbol_sch_text(&embedded, "Lib:C", "C4", "20250114");
        let viewer = sexp::parse(&viewer).unwrap();
        assert!(viewer
            .child("lib_symbols")
            .unwrap()
            .child("symbol")
            .is_some());
    }

    #[test]
    fn generated_per_reference_symbols_share_a_group_identity() {
        let d32 = sexp::parse(
            r#"(symbol "Calibrator:D32" (pin_names (offset 0.762) hide)
                (property "Reference" "D32" (at 0 5.08 0))
                (property "Value" "BLUE" (at 0 2.54 0))
                (property "LCSC" "C28310438" (at 0 0 0))
                (symbol "D32_0_1" (polyline (pts (xy 1.27 -1.27) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none))))
                (symbol "D32_1_1" (pin passive line (at 5.08 0 180) (length 3.81) (name "~") (number "1"))))"#,
        )
        .unwrap();
        let d33 = sexp::parse(
            r#"(symbol "Calibrator:D33" (pin_names (offset 0.762) hide)
                (property "Reference" "D33" (at 0 5.08 0))
                (property "Value" "BLUE" (at 0 2.54 0))
                (property "LCSC" "C28310438" (at 0 0 0))
                (symbol "D33_0_1" (polyline (pts (xy 1.27 -1.27) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none))))
                (symbol "D33_1_1" (pin passive line (at 5.08 0 180) (length 3.81) (name "~") (number "1"))))"#,
        )
        .unwrap();
        assert_eq!(normalize_symbol(&d32), normalize_symbol(&d33));
        assert_eq!(
            part_group_identity(&d32.properties()),
            part_group_identity(&d33.properties())
        );
    }

    #[test]
    fn exact_part_identity_ignores_descriptive_values() {
        let j5 = vec![
            ("Value".to_string(), "GPS ANTENNA".to_string()),
            ("LCSC".to_string(), "C3172723".to_string()),
            ("MPN".to_string(), "132289".to_string()),
        ];
        let j6 = vec![
            ("Value".to_string(), "INPUT 5V / 2.5V THRESHOLD".to_string()),
            ("LCSC".to_string(), "C3172723".to_string()),
            ("MPN".to_string(), "132289".to_string()),
        ];
        assert_eq!(part_group_identity(&j5), part_group_identity(&j6));
        assert_eq!(
            physical_part_symbol_identity(&SchData::default(), None, Some("Calibrator:SMA"), &j5),
            ""
        );
    }

    #[test]
    fn unqualified_values_still_separate_parts() {
        let resistor = vec![
            ("Value".to_string(), "1k".to_string()),
            ("Footprint".to_string(), "R_0603".to_string()),
        ];
        let other = vec![
            ("Value".to_string(), "10k".to_string()),
            ("Footprint".to_string(), "R_0603".to_string()),
        ];
        assert_ne!(part_group_identity(&resistor), part_group_identity(&other));
    }

    #[test]
    fn test_point_values_are_net_labels_not_part_identity() {
        let tp1 = vec![
            ("Value".to_string(), "FPGA_CRESET_N".to_string()),
            ("Footprint".to_string(), "Module:TestPoint_0p8".to_string()),
        ];
        let tp2 = vec![
            ("Value".to_string(), "FPGA_CDONE".to_string()),
            ("Footprint".to_string(), "Module:TestPoint_0p8".to_string()),
        ];
        assert_eq!(
            grouped_part_identity(
                "TP1",
                Some("Module:TP1"),
                Some("Module:TestPoint_0p8"),
                &tp1
            ),
            grouped_part_identity(
                "TP2",
                Some("Module:TP2"),
                Some("Module:TestPoint_0p8"),
                &tp2
            )
        );
        assert_ne!(part_group_identity(&tp1), part_group_identity(&tp2));
    }

    #[test]
    fn model_render_zoom_tracks_footprint_extent() {
        let small = sexp::parse(
            r#"(footprint "small" (layer "F.Cu")
                (pad "1" smd rect (at -0.5 0) (size 0.4 0.4) (layers "F.Cu"))
                (pad "2" smd rect (at 0.5 0) (size 0.4 0.4) (layers "F.Cu")))"#,
        )
        .unwrap();
        let large = sexp::parse(
            r#"(footprint "large" (layer "F.Cu")
                (pad "1" smd rect (at -5 0) (size 1 1) (layers "F.Cu"))
                (pad "2" smd rect (at 5 0) (size 1 1) (layers "F.Cu")))"#,
        )
        .unwrap();
        let zoom = |node: &Sexp| model_render_params(node)[1].parse::<f64>().unwrap();
        assert!(zoom(&small) > zoom(&large));
        assert!(zoom(&small) <= 0.95);
        assert!(zoom(&large) >= 0.35);
    }

    #[test]
    fn resolves_models_and_expands_vars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("3dmodels")).unwrap();
        std::fs::write(dir.path().join("3dmodels/R_0402.step"), b"step").unwrap();
        let resolver = Resolver::new(dir.path());
        let mut fp = sexp::parse(&BOARD_FP.replace(
            "(attr smd)",
            "(attr smd) (model \"${KICAD99_NOPE}/missing.wrl\")",
        ))
        .unwrap();
        let (refs, files) = resolve_models(&mut fp, &resolver);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs.iter().filter(|model| model.found).count(), 1);
        assert_eq!(files, vec![dir.path().join("3dmodels/R_0402.step")]);
        let text = fp.to_string();
        assert!(!text.contains("KIPRJMOD"));
        assert!(!text.contains("missing.wrl"));

        let mut vars = HashMap::new();
        vars.insert("A".to_string(), "/x".to_string());
        assert_eq!(expand_vars("${A}/b $(A) ${B}", &vars), "/x/b /x ${B}");
    }

    #[test]
    fn model_cache_identity_uses_model_content_not_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.step");
        let b = dir.path().join("b.step");
        std::fs::write(&a, b"same-step").unwrap();
        std::fs::write(&b, b"same-step").unwrap();

        let node = |name: &str, path: &Path| {
            sexp::parse(&format!(
                r#"(footprint "{name}" (layer "F.Cu") (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu")) (model "{}" (offset (xyz 1 2 3)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90))))"#,
                path.display()
            ))
            .unwrap()
        };

        let ida = model_cache_identity(&model_only_footprint(&node("A", &a)));
        let idb = model_cache_identity(&model_only_footprint(&node("B", &b)));
        assert_eq!(ida, idb);

        std::fs::write(&b, b"different-step").unwrap();
        let changed = model_cache_identity(&model_only_footprint(&node("B", &b)));
        assert_ne!(ida, changed);
    }

    #[test]
    fn classifies_part_fields() {
        assert_eq!(field_kind("LCSC Part #"), Some("lcsc"));
        assert_eq!(field_kind("JLCPCB Part"), Some("lcsc"));
        assert_eq!(field_kind("MPN"), Some("mpn"));
        assert_eq!(field_kind("Manufacturer_Part_Number"), Some("mpn"));
        assert_eq!(field_kind("Manufacturer"), Some("manufacturer"));
        assert_eq!(field_kind("Tolerance"), None);
    }
}
