use std::{
    collections::HashSet,
    ffi::OsString,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use include_dir::{include_dir, Dir};
use kicad_pcb_shared::{
    CheckResponse, FileEntry, HealthResponse, ManifestResponse, RevisionResponse, ServerEvent,
    SourceFile, SourcesResponse,
};
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::{
    process::Command,
    sync::{broadcast, mpsc, RwLock},
    time::{sleep, Duration},
};
use tower_http::trace::TraceLayer;
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, ZipWriter};

static VIEWER_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../static/kicad-viewer");

const TABS: &[&str] = &[
    "schematic",
    "pcb",
    "gerbers",
    "3d",
    "step",
    "bom",
    "libraries",
    "analysis",
    "panelization",
    "checks",
];

#[derive(Debug, Parser)]
#[command(name = "kicad-pcb")]
#[command(about = "Agent Portal KiCad PCB plugin runtime")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Doctor {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
    },
    Serve {
        #[arg(long)]
        port: u16,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long, default_value = "")]
        session: String,
    },
    Drc {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
    },
    Erc {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
    },
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ExportCommand {
    Gerbers {
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    Jlcpcb {
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Debug, Clone)]
struct AppState {
    cwd: PathBuf,
    session: String,
    warmed: Arc<RwLock<ViewerState>>,
    events: broadcast::Sender<ServerEvent>,
}

#[derive(Debug, Clone)]
struct ViewerState {
    manifest: ManifestResponse,
    sources: Vec<SourceFile>,
    source_revision: String,
    model_path: Option<PathBuf>,
    model_result: Option<serde_json::Value>,
    warmed_at_ms: u64,
}

#[derive(Debug)]
struct CommandOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kicad_pcb_server=info,tower_http=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Doctor { json, cwd } => {
            let cwd = cwd.canonicalize().context("canonicalize cwd")?;
            let response = serde_json::json!({
                "ok": true,
                "cwd": cwd,
                "tools": {
                    "kicad-cli": kicad_cli().is_some(),
                    "kikit": find_on_path("kikit").is_some(),
                },
                "tool_paths": {
                    "kicad-cli": kicad_cli(),
                },
                "versions": {
                    "kicad": tool_version(kicad_cli()).await,
                },
                "viewer_assets": true,
                "detected_files": detect_files(&cwd)?.into_iter().map(|item| item.path).collect::<Vec<_>>(),
                "tabs": TABS,
            });
            print_json_or_debug(json, &response)?;
        }
        Commands::Serve { port, cwd, session } => {
            let cwd = cwd.canonicalize().context("canonicalize cwd")?;
            let initial = warm_viewer_state(&cwd).await?;
            let (events, _) = broadcast::channel(128);
            let state = AppState {
                cwd,
                session,
                warmed: Arc::new(RwLock::new(initial)),
                events,
            };
            spawn_file_watcher(state.clone());
            let app = Router::new()
                .route("/", get(index))
                .route("/ws/events", get(events_ws))
                .route("/healthz", get(healthz))
                .route("/api/project", get(manifest))
                .route("/api/kicad/manifest", get(manifest))
                .route("/api/kicad/revision", get(revision))
                .route("/api/kicad/sources", get(sources))
                .route("/api/kicad/model.glb", get(model_glb))
                .route("/api/kicad/drc", get(drc))
                .route("/api/kicad/erc", get(erc))
                .route("/api/kicad/file", get(file))
                .route("/kicad-viewer/*path", get(viewer_asset))
                .layer(TraceLayer::new_for_http())
                .with_state(state);
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!(%addr, "serving KiCad PCB plugin");
            axum::serve(listener, app).await?;
        }
        Commands::Drc { json, cwd } => {
            let result = run_check(&cwd.canonicalize()?, "drc").await?;
            print_json_or_debug(json, &result)?;
            if !result.ok {
                std::process::exit(1);
            }
        }
        Commands::Erc { json, cwd } => {
            let result = run_check(&cwd.canonicalize()?, "erc").await?;
            print_json_or_debug(json, &result)?;
            if !result.ok {
                std::process::exit(1);
            }
        }
        Commands::Export { command } => match command {
            ExportCommand::Gerbers { cwd, out } => {
                let result = export_gerbers(&cwd.canonicalize()?, &out).await?;
                print_json_or_debug(true, &result)?;
                if !result["ok"].as_bool().unwrap_or(false) {
                    std::process::exit(1);
                }
            }
            ExportCommand::Jlcpcb { cwd, out } => {
                let result = export_jlcpcb(&cwd.canonicalize()?, &out).await?;
                print_json_or_debug(true, &result)?;
                if !result["ok"].as_bool().unwrap_or(false) {
                    std::process::exit(1);
                }
            }
        },
    }
    Ok(())
}

fn print_json_or_debug<T>(json: bool, value: &T) -> Result<()>
where
    T: serde::Serialize + std::fmt::Debug,
{
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{value:#?}");
    }
    Ok(())
}

async fn index(State(state): State<AppState>) -> Html<String> {
    let warmed = state.warmed.read().await.clone();
    Html(workbench_html(&state.cwd, &state.session, &warmed))
}

async fn events_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| event_socket(state, socket))
}

async fn event_socket(state: AppState, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();

    let initial = {
        let warmed = state.warmed.read().await;
        ServerEvent::Revision {
            revision: warmed.source_revision.clone(),
            previous_revision: None,
            warmed_at_ms: warmed.warmed_at_ms,
            reason: "initial".to_string(),
        }
    };
    if let Ok(text) = serde_json::to_string(&initial) {
        if sender.send(Message::Text(text)).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = receiver.next() => {
                if incoming.is_none() {
                    break;
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        let Ok(text) = serde_json::to_string(&event) else { continue };
                        if sender.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    let warmed = state.warmed.read().await;
    Json(HealthResponse {
        ok: true,
        plugin: "kicad-pcb".to_string(),
        kicad_cli: kicad_cli().is_some(),
        preloaded: true,
        source_revision: Some(warmed.source_revision.clone()),
        warmed_at_ms: Some(warmed.warmed_at_ms),
    })
}

async fn manifest(State(state): State<AppState>) -> Json<ManifestResponse> {
    Json(state.warmed.read().await.manifest.clone())
}

async fn revision(State(state): State<AppState>) -> Result<Json<RevisionResponse>, AppError> {
    let current = current_source_revision(&state.cwd)?;
    let warmed = state.warmed.read().await;
    Ok(Json(RevisionResponse {
        ok: true,
        changed: current != warmed.source_revision,
        revision: current,
        warmed_revision: Some(warmed.source_revision.clone()),
        warmed_at_ms: Some(warmed.warmed_at_ms),
    }))
}

async fn sources(State(state): State<AppState>) -> Result<Json<SourcesResponse>, AppError> {
    let warmed = refresh_viewer_state(&state).await?;
    Ok(Json(SourcesResponse {
        ok: true,
        revision: warmed.source_revision,
        sources: warmed.sources,
        warmed_at_ms: warmed.warmed_at_ms,
    }))
}

async fn drc(State(state): State<AppState>) -> Result<Json<CheckResponse>, AppError> {
    Ok(Json(run_check(&state.cwd, "drc").await?))
}

async fn erc(State(state): State<AppState>) -> Result<Json<CheckResponse>, AppError> {
    Ok(Json(run_check(&state.cwd, "erc").await?))
}

async fn model_glb(State(state): State<AppState>) -> Result<Response, AppError> {
    let warmed = refresh_viewer_state(&state).await?;
    if let Some(path) = warmed.model_path {
        if path.exists() {
            let bytes = tokio::fs::read(path).await?;
            return Ok((
                [
                    (header::CONTENT_TYPE, "model/gltf-binary"),
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                bytes,
            )
                .into_response());
        }
    }
    Ok((StatusCode::INTERNAL_SERVER_ERROR, Json(warmed.model_result)).into_response())
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: String,
    download: Option<String>,
}

async fn file(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Response, AppError> {
    let target = safe_rel(&state.cwd, &query.path)?;
    let bytes = tokio::fs::read(&target).await?;
    let mime = mime_guess::from_path(&target)
        .first_or_octet_stream()
        .to_string();
    let mut response = ([(header::CONTENT_TYPE, mime)], bytes).into_response();
    if query.download.is_some() {
        let value = format!(
            "attachment; filename=\"{}\"",
            target.file_name().unwrap_or_default().to_string_lossy()
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&value).map_err(|err| anyhow!(err))?,
        );
    }
    Ok(response)
}

async fn viewer_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let Some(file) = VIEWER_ASSETS.get_file(path.trim_start_matches('/')) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_guess::from_path(file.path())
        .first_or_octet_stream()
        .to_string();
    (
        [(header::CONTENT_TYPE, mime)],
        Body::from(file.contents().to_vec()),
    )
        .into_response()
}

async fn refresh_viewer_state(state: &AppState) -> Result<ViewerState> {
    let revision = current_source_revision(&state.cwd)?;
    {
        let warmed = state.warmed.read().await;
        if warmed.source_revision == revision {
            return Ok(warmed.clone());
        }
    }
    let refreshed = warm_viewer_state(&state.cwd).await?;
    let mut warmed = state.warmed.write().await;
    if warmed.source_revision != refreshed.source_revision {
        *warmed = refreshed;
    }
    Ok(warmed.clone())
}

fn spawn_file_watcher(state: AppState) {
    tokio::spawn(async move {
        if let Err(err) = watch_project_files(state).await {
            tracing::warn!(error = %err, "KiCad PCB file watcher stopped");
        }
    });
}

async fn watch_project_files(state: AppState) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cwd = state.cwd.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        NotifyConfig::default(),
    )?;
    watcher.watch(&cwd, RecursiveMode::Recursive)?;

    while let Some(result) = rx.recv().await {
        let event = match result {
            Ok(event) => event,
            Err(err) => {
                tracing::debug!(error = %err, "ignored file watch error");
                continue;
            }
        };
        if !is_interesting_event(&cwd, &event) {
            continue;
        }
        sleep(Duration::from_millis(150)).await;
        while let Ok(Ok(event)) = rx.try_recv() {
            if is_interesting_event(&cwd, &event) {
                sleep(Duration::from_millis(50)).await;
            }
        }
        let previous = {
            let warmed = state.warmed.read().await;
            warmed.source_revision.clone()
        };
        match refresh_viewer_state(&state).await {
            Ok(warmed) if warmed.source_revision != previous => {
                let _ = state.events.send(ServerEvent::Revision {
                    revision: warmed.source_revision,
                    previous_revision: Some(previous),
                    warmed_at_ms: warmed.warmed_at_ms,
                    reason: "watch".to_string(),
                });
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = %err, "failed to refresh KiCad PCB viewer state"),
        }
    }
    Ok(())
}

fn is_interesting_event(cwd: &Path, event: &notify::Event) -> bool {
    match event.kind {
        EventKind::Access(_) | EventKind::Other => return false,
        _ => {}
    }
    event.paths.iter().any(|path| {
        path.is_file()
            && is_project_file(cwd, path, true)
            && (kind_for(path).is_some()
                || path.file_name().and_then(|v| v.to_str()) == Some("fp-lib-table")
                || path.file_name().and_then(|v| v.to_str()) == Some("sym-lib-table"))
    })
}

async fn warm_viewer_state(cwd: &Path) -> Result<ViewerState> {
    let sources = kicad_sources(cwd)?;
    let source_revision = kicad_sources_revision(cwd, &sources)?;
    let mut manifest = manifest_for(cwd).await?;
    manifest.revision = now_ms().to_string();
    let cache_dir = cache_dir_for(cwd)?;
    tokio::fs::create_dir_all(&cache_dir).await?;
    let model_path = cache_dir.join(format!("{source_revision}.glb"));
    let mut model_result = None;
    if !model_path.exists() {
        let result = export_glb(cwd, &model_path).await?;
        model_result = Some(result.clone());
        if !result["ok"].as_bool().unwrap_or(false) && model_path.exists() {
            let _ = tokio::fs::remove_file(&model_path).await;
        }
    }
    Ok(ViewerState {
        manifest,
        sources,
        source_revision,
        model_path: model_path.exists().then_some(model_path),
        model_result,
        warmed_at_ms: now_ms(),
    })
}

async fn manifest_for(cwd: &Path) -> Result<ManifestResponse> {
    let kicad = kicad_cli();
    let mut warnings = Vec::new();
    if kicad.is_none() {
        warnings.push(
            "kicad-cli is not available; DRC/ERC/export/BOM/3D generation are disabled."
                .to_string(),
        );
    }
    Ok(ManifestResponse {
        root: cwd.display().to_string(),
        revision: now_ms().to_string(),
        files: detect_files(cwd)?,
        warnings,
        kicad_version: tool_version(kicad.clone()).await,
        kicad_cli: kicad,
    })
}

fn detect_files(cwd: &Path) -> Result<Vec<FileEntry>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(cwd).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() || !is_project_file(cwd, path, false) {
            continue;
        }
        let Some(kind) = kind_for(path) else { continue };
        let meta = path.metadata()?;
        files.push(FileEntry {
            path: rel(cwd, path)?,
            kind: kind.to_string(),
            mime_type: mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string(),
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
        });
    }
    files.sort_by(|a, b| (&a.kind, &a.path).cmp(&(&b.kind, &b.path)));
    Ok(files)
}

fn kicad_sources(cwd: &Path) -> Result<Vec<SourceFile>> {
    let mut sources = Vec::new();
    for entry in WalkDir::new(cwd).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() || !is_project_file(cwd, path, true) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        let suffix = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        let wanted = matches!(
            suffix,
            "kicad_pro" | "kicad_pcb" | "kicad_sch" | "kicad_sym" | "kicad_mod" | "kicad_wks"
        ) || matches!(name, "fp-lib-table" | "sym-lib-table");
        if wanted {
            sources.push(SourceFile {
                filename: rel(cwd, path)?,
                content: std::fs::read_to_string(path).unwrap_or_else(|_| String::new()),
            });
        }
    }
    sources.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(sources)
}

fn kicad_sources_revision(cwd: &Path, sources: &[SourceFile]) -> Result<String> {
    let latest = sources
        .iter()
        .filter_map(|source| std::fs::metadata(cwd.join(&source.filename)).ok())
        .map(|meta| mtime_ms(&meta))
        .max()
        .unwrap_or_else(now_ms);
    let mut digest = Sha256::new();
    for source in sources {
        digest.update(source.filename.as_bytes());
        digest.update([0]);
        digest.update(source.content.as_bytes());
        digest.update([0]);
    }
    Ok(format!("{latest}-{:x}", digest.finalize())[..26].to_string())
}

fn current_source_revision(cwd: &Path) -> Result<String> {
    let sources = kicad_sources(cwd)?;
    kicad_sources_revision(cwd, &sources)
}

fn kind_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "kicad_pro" => "project",
        "kicad_pcb" => "pcb",
        "kicad_sch" => "schematic",
        "kicad_mod" => "footprint",
        "kicad_sym" => "symbol",
        "kicad_wks" => "worksheet",
        "step" | "stp" | "glb" => "model",
        "gbr" | "gtl" | "gbl" | "gts" | "gbs" | "gto" | "gbo" | "gtp" | "gbp" | "gta" | "gba"
        | "gm1" | "gko" | "drl" | "zip" => "gerber",
        "csv" => "bom",
        "xml" => "netlist",
        _ => return None,
    })
}

fn is_project_file(cwd: &Path, path: &Path, active_source: bool) -> bool {
    let Ok(rel) = path.strip_prefix(cwd) else {
        return false;
    };
    let discovery_ignored: HashSet<&str> = [
        ".git",
        ".portal",
        ".runtime",
        "__pycache__",
        "node_modules",
        "target",
    ]
    .into_iter()
    .collect();
    let active_ignored: HashSet<&str> = [
        ".git",
        ".portal",
        ".runtime",
        "__pycache__",
        "node_modules",
        "target",
        "build",
        "dist",
        "tmp",
        "temp",
        "backup",
        "backups",
    ]
    .into_iter()
    .collect();
    let ignored = if active_source {
        &active_ignored
    } else {
        &discovery_ignored
    };
    if rel
        .components()
        .any(|part| ignored.contains(part.as_os_str().to_string_lossy().as_ref()))
    {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ![".bak", ".backup", ".orig", ".old", ".tmp", "~"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

async fn run_check(cwd: &Path, kind: &str) -> Result<CheckResponse> {
    let Some(cli) = kicad_cli() else {
        return Ok(CheckResponse {
            ok: false,
            source: None,
            command: vec![],
            stdout: String::new(),
            stderr: String::new(),
            report: String::new(),
            message: Some("kicad-cli is required".to_string()),
            tool: None,
        });
    };
    let source = if kind == "drc" {
        pick_one(cwd, "kicad_pcb")?
    } else {
        pick_one(cwd, "kicad_sch")?
    };
    let Some(source) = source else {
        return Ok(CheckResponse {
            ok: false,
            source: None,
            command: vec![],
            stdout: String::new(),
            stderr: String::new(),
            report: String::new(),
            message: Some(format!(
                "Select a single KiCad {kind} source or configure .kicad-pcb.json"
            )),
            tool: Some(cli),
        });
    };
    let tmp = TempDir::new()?;
    let report = tmp.path().join(format!("{kind}.json"));
    let mut args: Vec<OsString> = if kind == "drc" {
        vec![
            "pcb".into(),
            "drc".into(),
            "--format".into(),
            "json".into(),
            "--output".into(),
            report.as_os_str().into(),
        ]
    } else {
        vec![
            "sch".into(),
            "erc".into(),
            "--format".into(),
            "json".into(),
            "--output".into(),
            report.as_os_str().into(),
        ]
    };
    if kind == "drc" {
        args.push("--exit-code-violations".into());
        args.push("--schematic-parity".into());
    } else {
        args.push("--exit-code-violations".into());
    }
    args.push(source.as_os_str().into());
    let output = run_command(&cli, &args, cwd).await?;
    let report_text = tokio::fs::read_to_string(&report).await.unwrap_or_default();
    Ok(CheckResponse {
        ok: output.status == 0,
        source: Some(rel(cwd, &source)?),
        command: std::iter::once(cli.clone())
            .chain(args.iter().map(|v| v.to_string_lossy().into_owned()))
            .collect(),
        stdout: output.stdout,
        stderr: output.stderr,
        report: report_text,
        message: None,
        tool: Some(cli),
    })
}

async fn export_glb(cwd: &Path, out: &Path) -> Result<serde_json::Value> {
    let Some(cli) = kicad_cli() else {
        return Ok(serde_json::json!({"ok": false, "message": "kicad-cli is required"}));
    };
    let Some(board) = pick_one(cwd, "kicad_pcb")? else {
        return Ok(
            serde_json::json!({"ok": false, "message": "Select a single .kicad_pcb or set pcb in .kicad-pcb.json"}),
        );
    };
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let args: Vec<OsString> = [
        "pcb",
        "export",
        "glb",
        "--force",
        "--subst-models",
        "--include-tracks",
        "--include-pads",
        "--include-silkscreen",
        "--include-soldermask",
        "--output",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([out.as_os_str().into(), board.as_os_str().into()])
    .collect();
    let output = run_command(&cli, &args, cwd).await?;
    Ok(serde_json::json!({
        "ok": output.status == 0,
        "board": rel(cwd, &board)?,
        "out": out,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

async fn export_gerbers(cwd: &Path, out: &Path) -> Result<serde_json::Value> {
    export_fab(cwd, out, false).await
}

async fn export_jlcpcb(cwd: &Path, out: &Path) -> Result<serde_json::Value> {
    export_fab(cwd, out, true).await
}

async fn export_fab(cwd: &Path, out: &Path, jlcpcb: bool) -> Result<serde_json::Value> {
    let Some(cli) = kicad_cli() else {
        return Ok(serde_json::json!({"ok": false, "message": "kicad-cli is required"}));
    };
    let Some(board) = pick_one(cwd, "kicad_pcb")? else {
        return Ok(
            serde_json::json!({"ok": false, "message": "Select a single .kicad_pcb or set pcb in .kicad-pcb.json"}),
        );
    };
    let tmp = TempDir::new()?;
    let output_dir = format!("{}/", tmp.path().display());
    let gerber = run_command(
        &cli,
        &[
            "pcb".into(),
            "export".into(),
            "gerbers".into(),
            "--output".into(),
            output_dir.clone().into(),
            board.as_os_str().into(),
        ],
        cwd,
    )
    .await?;
    let drill = run_command(
        &cli,
        &[
            "pcb".into(),
            "export".into(),
            "drill".into(),
            "--output".into(),
            output_dir.into(),
            board.as_os_str().into(),
        ],
        cwd,
    )
    .await?;
    let mut status = gerber.status | drill.status;
    let mut stdout = format!("{}{}", gerber.stdout, drill.stdout);
    let mut stderr = format!("{}{}", gerber.stderr, drill.stderr);
    if jlcpcb {
        if let Some(schematic) = pick_one(cwd, "kicad_sch")? {
            let bom = tmp.path().join(format!(
                "BOM_{}.csv",
                board.file_stem().unwrap().to_string_lossy()
            ));
            let bom_out = run_command(
                &cli,
                &[
                    "sch".into(),
                    "export".into(),
                    "bom".into(),
                    "--output".into(),
                    bom.as_os_str().into(),
                    "--fields".into(),
                    "Value,Reference,Footprint,LCSC".into(),
                    "--labels".into(),
                    "Comment,Designator,Footprint,LCSC Part #".into(),
                    "--sort-field".into(),
                    "Reference".into(),
                    "--exclude-dnp".into(),
                    schematic.as_os_str().into(),
                ],
                cwd,
            )
            .await?;
            status |= bom_out.status;
            stdout.push_str(&bom_out.stdout);
            stderr.push_str(&bom_out.stderr);
            let raw = tmp.path().join(format!(
                "CPL_kicad_{}.csv",
                board.file_stem().unwrap().to_string_lossy()
            ));
            let cpl = tmp.path().join(format!(
                "CPL_{}.csv",
                board.file_stem().unwrap().to_string_lossy()
            ));
            let pos = run_command(
                &cli,
                &[
                    "pcb".into(),
                    "export".into(),
                    "pos".into(),
                    "--output".into(),
                    raw.as_os_str().into(),
                    "--format".into(),
                    "csv".into(),
                    "--units".into(),
                    "mm".into(),
                    "--side".into(),
                    "both".into(),
                    "--exclude-dnp".into(),
                    board.as_os_str().into(),
                ],
                cwd,
            )
            .await?;
            status |= pos.status;
            stdout.push_str(&pos.stdout);
            stderr.push_str(&pos.stderr);
            if raw.exists() {
                convert_position_csv(&raw, &cpl)?;
            }
        }
    }
    tokio::fs::create_dir_all(out).await?;
    let mut copied = Vec::new();
    for entry in std::fs::read_dir(tmp.path())? {
        let path = entry?.path();
        if path.is_file() && (is_gerber_artifact(&path) || (jlcpcb && is_manufacturing_csv(&path)))
        {
            let target = out.join(path.file_name().unwrap());
            std::fs::copy(&path, &target)?;
            copied.push(target);
        }
    }
    copied.sort();
    let suffix = if jlcpcb { "jlcpcb" } else { "gerbers" };
    let zip = out.join(format!(
        "{}-{suffix}.zip",
        board.file_stem().unwrap().to_string_lossy()
    ));
    zip_artifacts(&zip, &copied)?;
    Ok(serde_json::json!({
        "ok": status == 0,
        "board": rel(cwd, &board)?,
        "out": out,
        "zip": zip,
        "artifacts": copied,
        "stdout": stdout,
        "stderr": stderr,
    }))
}

fn convert_position_csv(source: &Path, destination: &Path) -> Result<()> {
    let text = std::fs::read_to_string(source)?;
    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| anyhow!("empty position CSV"))?;
    let columns: Vec<&str> = header.split(',').collect();
    let idx = |name: &str| {
        columns
            .iter()
            .position(|col| *col == name)
            .ok_or_else(|| anyhow!("missing {name}"))
    };
    let ref_i = idx("Ref")?;
    let x_i = idx("PosX")?;
    let y_i = idx("PosY")?;
    let rot_i = idx("Rot")?;
    let side_i = idx("Side")?;
    let mut out = String::from("Designator,Mid X,Mid Y,Layer,Rotation\n");
    for line in lines {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() <= side_i {
            continue;
        }
        let layer = if matches!(
            cols[side_i].trim().to_ascii_lowercase().as_str(),
            "top" | "front"
        ) {
            "T"
        } else {
            "B"
        };
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            cols[ref_i], cols[x_i], cols[y_i], layer, cols[rot_i]
        ));
    }
    std::fs::write(destination, out)?;
    Ok(())
}

fn zip_artifacts(zip_path: &Path, artifacts: &[PathBuf]) -> Result<()> {
    let file = std::fs::File::create(zip_path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for artifact in artifacts {
        let name = artifact.file_name().unwrap().to_string_lossy();
        zip.start_file(name, options)?;
        let bytes = std::fs::read(artifact)?;
        zip.write_all(&bytes)?;
    }
    zip.finish()?;
    Ok(())
}

fn pick_one(cwd: &Path, extension: &str) -> Result<Option<PathBuf>> {
    let mut matches = Vec::new();
    for entry in WalkDir::new(cwd).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.is_file()
            && is_project_file(cwd, path, true)
            && path.extension().and_then(|v| v.to_str()) == Some(extension)
        {
            matches.push(path.to_path_buf());
        }
    }
    matches.sort();
    Ok((matches.len() == 1).then(|| matches.remove(0)))
}

async fn run_command(program: &str, args: &[OsString], cwd: &Path) -> Result<CommandOutput> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("run {program}"))?;
    Ok(CommandOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn tool_version(tool: Option<String>) -> Option<String> {
    let tool = tool?;
    let output = run_command(&tool, &["--version".into()], Path::new("."))
        .await
        .ok()?;
    if output.status != 0 {
        return None;
    }
    output
        .stdout
        .lines()
        .next()
        .or_else(|| output.stderr.lines().next())
        .map(str::to_string)
}

fn kicad_cli() -> Option<String> {
    std::env::var("KICAD_PCB_KICAD_CLI")
        .ok()
        .or_else(|| std::env::var("BACKPLANE_KICAD_CLI").ok())
        .or_else(|| std::env::var("KICAD_CLI").ok())
        .or_else(|| find_on_path("kicad-cli"))
}

fn find_on_path(name: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|path| path.join(name))
        .find(|path| path.exists())
        .map(|path| path.display().to_string())
}

fn cache_dir_for(cwd: &Path) -> Result<PathBuf> {
    let mut digest = Sha256::new();
    digest.update(cwd.display().to_string().as_bytes());
    let hash = format!("{:x}", digest.finalize());
    Ok(plugin_dir()?
        .join(".portal")
        .join("cache")
        .join(&hash[..16]))
}

fn plugin_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("AGENT_PORTAL_PLUGIN_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(std::env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("could not resolve plugin dir"))?
        .to_path_buf())
}

fn safe_rel(cwd: &Path, requested: &str) -> Result<PathBuf> {
    let candidate = cwd.join(requested).canonicalize()?;
    if !candidate.starts_with(cwd) {
        return Err(anyhow!("path escapes project root"));
    }
    Ok(candidate)
}

fn rel(cwd: &Path, path: &Path) -> Result<String> {
    Ok(path.strip_prefix(cwd)?.to_string_lossy().replace('\\', "/"))
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_else(now_ms)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_gerber_artifact(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default(),
        "gbr"
            | "gtl"
            | "gbl"
            | "gts"
            | "gbs"
            | "gto"
            | "gbo"
            | "gtp"
            | "gbp"
            | "gta"
            | "gba"
            | "gm1"
            | "gko"
            | "drl"
    )
}

fn is_manufacturing_csv(path: &Path) -> bool {
    path.file_name()
        .and_then(|v| v.to_str())
        .map(|name| {
            name.starts_with("BOM_")
                || (name.starts_with("CPL_") && !name.starts_with("CPL_kicad_"))
        })
        .unwrap_or(false)
}

fn workbench_html(cwd: &Path, session: &str, warmed: &ViewerState) -> String {
    let tabs = TABS
        .iter()
        .copied()
        .map(|tab| format!("<button data-tab='{tab}'>{}</button>", title(tab)))
        .collect::<String>();
    let files = warmed
        .manifest
        .files
        .iter()
        .map(|file| {
            format!(
                "<li><code>{}</code> <span class='muted'>{}</span></li>",
                escape(&file.path),
                escape(&file.kind)
            )
        })
        .collect::<String>();
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>KiCad PCB · Agent Portal</title>
<style>
body{{margin:0;background:#16161e;color:#c0caf5;font-family:Inter,ui-sans-serif,system-ui,sans-serif}}header{{padding:14px 16px;border-bottom:1px solid #3b4261;background:#1a1b26}}h1{{margin:0;font-size:18px;color:#e6e9f5}}.muted{{color:#9aa5ce}}.tabs{{display:flex;gap:4px;flex-wrap:wrap;padding:8px;background:#1f2335;position:sticky;top:0;z-index:2}}.tabs button{{border:1px solid #3b4261;color:#c0caf5;background:#24283b;padding:7px 10px;border-radius:6px;cursor:pointer}}.tabs button.active{{background:#7aa2f7;color:#10131d;border-color:#7aa2f7}}main{{padding:14px}}section{{display:none;min-height:55vh}}section.active{{display:block}}.card{{border:1px solid #3b4261;border-radius:8px;background:#1f2335;padding:12px}}iframe{{width:100%;height:70vh;border:1px solid #3b4261;border-radius:8px;background:#11131d}}pre{{white-space:pre-wrap;overflow:auto;background:#11131d;padding:12px;border-radius:6px}}code{{color:#7dcfff}}
</style></head><body>
<header><h1>KiCad PCB Workbench</h1><div class="muted">Session {session} · {cwd}</div></header>
<nav class="tabs">{tabs}</nav><main>
<section id="schematic"><div class="card"><h2>Schematic</h2><iframe class="native-viewer" data-kind="schematic" src="/kicad-viewer/runtime.html"></iframe></div></section>
<section id="pcb"><div class="card"><h2>PCB</h2><iframe class="native-viewer" data-kind="pcb" src="/kicad-viewer/runtime.html"></iframe></div></section>
<section id="3d"><div class="card"><h2>3D Board</h2><iframe class="model-viewer" data-kind="model" src="/kicad-viewer/runtime.html"></iframe></div></section>
<section id="checks"><div class="card"><h2>Checks</h2><pre id="checksOut">Open checks...</pre></div></section>
<section id="gerbers"><div class="card"><h2>Gerbers</h2><ul>{files}</ul></div></section>
<section id="step"><div class="card"><h2>STEP</h2><ul>{files}</ul></div></section>
<section id="bom"><div class="card"><h2>BOM</h2><ul>{files}</ul></div></section>
<section id="libraries"><div class="card"><h2>Libraries</h2><ul>{files}</ul></div></section>
<section id="analysis"><div class="card"><h2>Analysis</h2><p class="muted">Analysis workflow is pending the Rust port.</p></div></section>
<section id="panelization"><div class="card"><h2>Panelization</h2><p class="muted">Panelization workflow is pending the Rust port.</p></div></section>
</main><script>
let sourceSnapshot;
let sourceSnapshotPromise;
let activeTab = "schematic";
let refreshInFlight = false;
const loadSources = async () => {{
  if (sourceSnapshot) return sourceSnapshot;
  sourceSnapshotPromise ||= fetch("/api/kicad/sources")
    .then(r => r.json())
    .then(payload => {{
      sourceSnapshot = payload;
      sourceSnapshotPromise = undefined;
      return payload;
    }}, err => {{
      sourceSnapshotPromise = undefined;
      throw err;
    }});
  return sourceSnapshotPromise;
}};
const modelUrl = revision => `/api/kicad/model.glb?rev=${{encodeURIComponent(revision || "current")}}`;
const viewerFrames = () => document.querySelectorAll("iframe.native-viewer, iframe.model-viewer");
const postSnapshot = async frame => {{
  if (frame.dataset.kind === "model") {{
    const payload = await loadSources();
    frame.contentWindow?.postMessage({{type:"backplane-snapshot", kind:"model", url:modelUrl(payload.revision), active:true}}, location.origin);
    return;
  }}
  const payload = await loadSources();
  frame.contentWindow?.postMessage({{type:"backplane-snapshot", kind:"native", context:frame.dataset.kind, revision:payload.revision, sources:payload.sources, active:true}}, location.origin);
}};
window.addEventListener("message", event => {{
  if (event.origin !== location.origin) return;
  if (event.data?.type === "backplane-runtime-ready") {{
    const frame = [...viewerFrames()].find(item => item.contentWindow === event.source);
    if (frame) void postSnapshot(frame);
  }}
}});
const loadChecks = async () => {{
  const [drc, erc] = await Promise.all([fetch("/api/kicad/drc").then(r=>r.json()), fetch("/api/kicad/erc").then(r=>r.json())]);
  checksOut.textContent = JSON.stringify({{drc, erc}}, null, 2);
}};
const refreshPane = async () => {{
  if (refreshInFlight) return;
  refreshInFlight = true;
  try {{
    const status = await fetch("/api/kicad/revision").then(r => r.json());
    if (!sourceSnapshot || status.changed || status.revision !== sourceSnapshot.revision) {{
      sourceSnapshot = undefined;
      await loadSources();
      viewerFrames().forEach(frame => void postSnapshot(frame));
      if (activeTab === "checks") await loadChecks();
    }}
  }} finally {{ refreshInFlight = false; }}
}};
const applyRevision = async event => {{
  if (!event?.revision || sourceSnapshot?.revision === event.revision) return;
  sourceSnapshot = undefined;
  await loadSources();
  // The retained native viewer prepares replacement sources internally and
  // keeps the previous canvas alive until the new parse/render is usable.
  viewerFrames().forEach(frame => void postSnapshot(frame));
  if (activeTab === "checks") await loadChecks();
}};
const connectEvents = () => {{
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const socket = new WebSocket(`${{proto}}//${{location.host}}/ws/events`);
  socket.onmessage = event => {{
    try {{
      const payload = JSON.parse(event.data);
      if (payload.type === "Revision") void applyRevision(payload);
    }} catch (err) {{
      console.warn("KiCad PCB event decode failed", err);
    }}
  }};
  socket.onclose = () => setTimeout(connectEvents, 1000);
  socket.onerror = () => socket.close();
}};
const setTab = id => {{
  activeTab = id;
  document.querySelectorAll("section").forEach(el => el.classList.toggle("active", el.id === id));
  document.querySelectorAll(".tabs button").forEach(el => el.classList.toggle("active", el.dataset.tab === id));
  document.querySelectorAll(`#${{CSS.escape(id)}} iframe.native-viewer, #${{CSS.escape(id)}} iframe.model-viewer`).forEach(frame => void postSnapshot(frame));
  if (id === "checks") void loadChecks();
}};
document.querySelectorAll(".tabs button").forEach(b => b.onclick = () => setTab(b.dataset.tab));
setTab(document.getElementById(location.hash.slice(1)) ? location.hash.slice(1) : "schematic");
loadSources().then(() => viewerFrames().forEach(frame => void postSnapshot(frame))).catch(err => console.warn("KiCad PCB preload failed", err));
connectEvents();
setInterval(refreshPane, 30000);
document.addEventListener("visibilitychange", () => {{ if (!document.hidden) void refreshPane(); }});
</script></body></html>"#,
        session = escape(session),
        cwd = escape(&cwd.display().to_string()),
        tabs = tabs,
        files = files,
    )
}

fn title(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Debug)]
struct AppError(anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self(value.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"ok": false, "message": self.0.to_string()})),
        )
            .into_response()
    }
}
