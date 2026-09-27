mod artifacts;
mod audits;
mod bom;
mod jobs;
mod library;
mod profile;
mod quality;
mod revision;
mod sexp;
mod sexpr;
mod thumbnails;
mod watch;
use std::{
    collections::{HashMap, HashSet},
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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::{broadcast, RwLock};
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
    Setup {
        #[arg(long)]
        install_kicad_tools: bool,
        #[arg(long, default_value = "agent")]
        kicad_tools_extra: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
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
        #[arg(long)]
        project: Option<String>,
    },
    Erc {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        project: Option<String>,
    },
    /// Layout-quality checks (kct rules + in-plugin audits) against the
    /// workspace quality profile.
    Quality {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        project: Option<String>,
    },
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
    Kct {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(last = true, trailing_var_arg = true)]
        args: Vec<OsString>,
    },
    Tool {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(last = true, trailing_var_arg = true)]
        args: Vec<OsString>,
    },
}

#[derive(Debug, Subcommand)]
enum ExportCommand {
    Gerbers {
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        project: Option<String>,
    },
    Jlcpcb {
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct AppState {
    cwd: PathBuf,
    session: String,
    projects: Vec<ProjectContext>,
    warmed: Arc<RwLock<HashMap<String, ViewerState>>>,
    events: broadcast::Sender<ServerEvent>,
    builds: jobs::BuildEngine,
}

#[derive(Debug, Clone)]
struct ProjectContext {
    id: String,
    name: String,
    root: PathBuf,
    config: ProjectConfig,
    shared_libraries: LibraryConfig,
    /// Repository root (the workspace cwd).
    repo_root: PathBuf,
    /// Other projects' roots nested inside this one (excluded from its sources).
    excluded_roots: Vec<PathBuf>,
    /// Resolved layout-quality profile and settings.
    quality: quality::QualitySettings,
}

#[derive(Debug, Clone)]
struct ViewerState {
    manifest: ManifestResponse,
    sources: Vec<SourceFile>,
    source_revision: String,
    warmed_at_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ProjectQuery {
    project: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceConfig {
    version: Option<u32>,
    default_project: Option<String>,
    libraries: Option<LibraryConfig>,
    projects: Option<Vec<ProjectConfig>>,
    build: Option<jobs::BuildConfig>,
    /// Workspace-relative path to the layout-quality profile YAML.
    quality_profile: Option<PathBuf>,
    quality: Option<quality::QualityConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectConfig {
    id: String,
    name: Option<String>,
    root: PathBuf,
    kicad: Option<KicadFilesConfig>,
    artifacts: Option<ArtifactConfig>,
    libraries: Option<LibraryConfig>,
    manufacturer: Option<serde_json::Value>,
    inherit_libraries: Option<bool>,
    /// Project-root-relative quality profile; overrides the workspace one.
    quality_profile: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct KicadFilesConfig {
    project: Option<PathBuf>,
    schematic: Option<PathBuf>,
    pcb: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct ArtifactConfig {
    fab: Option<PathBuf>,
    checks: Option<PathBuf>,
    gerbers: Option<PathBuf>,
    jlcpcb: Option<PathBuf>,
    docs: Option<PathBuf>,
    /// Copy each completed build into the artifact dirs automatically.
    #[serde(rename = "autoPublish")]
    auto_publish: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct LibraryConfig {
    symbols: Option<Vec<PathBuf>>,
    footprints: Option<Vec<PathBuf>>,
    models: Option<Vec<PathBuf>>,
    datasheets: Option<Vec<PathBuf>>,
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
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "kicad_pcb_rs=info,kicad_pcb_server=info,tower_http=warn".into()
            }),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Setup {
            install_kicad_tools,
            kicad_tools_extra,
            force,
            json,
        } => {
            let mut response = serde_json::json!({
                "ok": true,
                "runtime_dir": plugin_dir()?.join(".runtime"),
                "kicad_cli": kicad_cli(),
                "kicad_version": tool_version(kicad_cli()).await,
                "kct": kct_cli(),
                "kct_version": kct_version().await,
            });
            if install_kicad_tools {
                let install = install_kicad_tools_runtime(force, &kicad_tools_extra).await?;
                response["ok"] = serde_json::Value::Bool(
                    response["ok"].as_bool().unwrap_or(false)
                        && install["ok"].as_bool().unwrap_or(false),
                );
                response["kicad_tools_install"] = install;
                response["kct"] = serde_json::json!(kct_cli());
                response["kct_version"] = serde_json::json!(kct_version().await);
            }
            print_json_or_debug(json, &response)?;
            if !response["ok"].as_bool().unwrap_or(false) {
                std::process::exit(1);
            }
        }
        Commands::Doctor { json, cwd } => {
            let cwd = cwd.canonicalize().context("canonicalize cwd")?;
            let projects = match project_contexts(&cwd) {
                Ok(projects) => projects,
                Err(err) if json => {
                    let response = serde_json::json!({
                        "ok": false,
                        "cwd": cwd,
                        "message": err.to_string(),
                    });
                    print_json_or_debug(true, &response)?;
                    std::process::exit(1);
                }
                Err(err) => return Err(err),
            };
            let mut library_warnings = projects
                .iter()
                .flat_map(|project| library_warnings(&cwd, project))
                .collect::<Vec<_>>();
            library_warnings.sort();
            library_warnings.dedup();
            let response = serde_json::json!({
                "ok": true,
                "cwd": cwd,
                "projects": projects.iter().map(project_summary).collect::<Vec<_>>(),
                "tools": {
                    "kicad-cli": kicad_cli().is_some(),
                    "kikit": find_on_path("kikit").is_some(),
                    "kct": kct_cli().is_some(),
                },
                "tool_paths": {
                    "kicad-cli": kicad_cli(),
                    "managed_kicad-cli": managed_kicad_cli(),
                    "kct": kct_cli(),
                    "managed_kct": managed_kct_cli().ok().flatten(),
                },
                "runtime": {
                    "server": "rust",
                    "binary": std::env::current_exe().ok(),
                    "version": env!("CARGO_PKG_VERSION"),
                    "runtime_dir": plugin_dir().ok().map(|dir| dir.join(".runtime")),
                },
                "versions": {
                    "kicad": tool_version(kicad_cli()).await,
                    "kicad-tools": kct_version().await,
                },
                "viewer_assets": true,
                "detected_files": detect_files(&cwd)?.into_iter().map(|item| item.path).collect::<Vec<_>>(),
                "warnings": library_warnings,
                "tabs": TABS,
                "build": artifacts::doctor_report(
                    &projects,
                    tool_version(kicad_cli()).await.as_deref(),
                    kct_version().await.as_deref(),
                    workspace_config(&cwd)?.build.as_ref(),
                ),
            });
            print_json_or_debug(json, &response)?;
        }
        Commands::Serve { port, cwd, session } => {
            let cwd = cwd.canonicalize().context("canonicalize cwd")?;
            let projects = project_contexts(&cwd)?;
            let default_project = projects
                .first()
                .ok_or_else(|| anyhow!("no KiCad PCB projects found"))?
                .clone();
            let initial = warm_viewer_state(&default_project).await?;
            let mut warmed = HashMap::new();
            warmed.insert(default_project.id.clone(), initial);
            let (events, _) = broadcast::channel(128);
            let builds = jobs::BuildEngine::new(
                jobs::BuildSettings::from_config(workspace_config(&cwd)?.build.as_ref()),
                projects.clone(),
                tool_version(kicad_cli()).await,
                kct_version().await,
                events.clone(),
            );
            builds.request(&default_project);
            let state = AppState {
                cwd,
                session,
                projects,
                warmed: Arc::new(RwLock::new(warmed)),
                events,
                builds,
            };
            watch::spawn(state.clone());
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
                .route("/api/kicad/quality", get(quality_endpoint))
                .route("/api/kicad/file", get(file))
                .route("/api/kicad/bom", get(bom::endpoint))
                .route("/api/kicad/libraries", get(libraries_endpoint))
                .merge(library::routes())
                .route("/kicad-viewer/*path", get(viewer_asset))
                .merge(jobs::routes())
                .layer(TraceLayer::new_for_http())
                .with_state(state);
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!(%addr, "serving KiCad PCB plugin");
            axum::serve(listener, app).await?;
        }
        Commands::Drc { json, cwd, project } => {
            let cwd = cwd.canonicalize()?;
            let project = cli_project_context(&cwd, project.as_deref())?;
            let result = run_check(&project, "drc").await?;
            print_json_or_debug(json, &result)?;
            if !result.ok {
                std::process::exit(1);
            }
        }
        Commands::Erc { json, cwd, project } => {
            let cwd = cwd.canonicalize()?;
            let project = cli_project_context(&cwd, project.as_deref())?;
            let result = run_check(&project, "erc").await?;
            print_json_or_debug(json, &result)?;
            if !result.ok {
                std::process::exit(1);
            }
        }
        Commands::Quality { json, cwd, project } => {
            let cwd = cwd.canonicalize()?;
            let project = cli_project_context(&cwd, project.as_deref())?;
            let result = quality::run_check(&project).await?;
            if json {
                print_json_or_debug(true, &result)?;
            } else {
                println!("{}", result.stdout);
                let report: serde_json::Value =
                    serde_json::from_str(&result.report).unwrap_or_default();
                if let Some(rules) = report
                    .pointer("/summary/by_rule")
                    .and_then(|v| v.as_object())
                {
                    for (rule, info) in rules {
                        println!(
                            "  {:>5}  {:<8} {rule}",
                            info["count"],
                            info["severity"].as_str().unwrap_or_default()
                        );
                    }
                }
            }
            if !result.ok {
                std::process::exit(1);
            }
        }
        Commands::Export { command } => match command {
            ExportCommand::Gerbers { cwd, out, project } => {
                let cwd = cwd.canonicalize()?;
                let project = cli_project_context(&cwd, project.as_deref())?;
                let out = export_out(&project, &out, "gerbers");
                let result = export_fab(&project, &out, false).await?;
                print_json_or_debug(true, &result)?;
                if !result["ok"].as_bool().unwrap_or(false) {
                    std::process::exit(1);
                }
            }
            ExportCommand::Jlcpcb { cwd, out, project } => {
                let cwd = cwd.canonicalize()?;
                let project = cli_project_context(&cwd, project.as_deref())?;
                let out = export_out(&project, &out, "jlcpcb");
                let result = export_fab(&project, &out, true).await?;
                print_json_or_debug(true, &result)?;
                if !result["ok"].as_bool().unwrap_or(false) {
                    std::process::exit(1);
                }
            }
        },
        Commands::Kct { json, cwd, args } => {
            let cwd = cwd.canonicalize()?;
            let Some(tool) = kct_cli() else {
                let response = serde_json::json!({
                    "ok": false,
                    "message": "kicad-tools is required. Run `kicad-pcb setup --install-kicad-tools`.",
                    "tool": null,
                });
                print_json_or_debug(true, &response)?;
                std::process::exit(1);
            };
            let pass_args = if args.is_empty() {
                vec![OsString::from("--help")]
            } else {
                args
            };
            let output = run_command(&tool, &pass_args, &cwd).await?;
            let response = serde_json::json!({
                "ok": output.status == 0,
                "tool": tool,
                "command": std::iter::once(OsString::from(&tool)).chain(pass_args.clone()).map(|item| item.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                "cwd": cwd,
                "stdout": output.stdout,
                "stderr": output.stderr,
            });
            if json {
                print_json_or_debug(true, &response)?;
            } else {
                print!("{}", response["stdout"].as_str().unwrap_or_default());
                eprint!("{}", response["stderr"].as_str().unwrap_or_default());
            }
            if !response["ok"].as_bool().unwrap_or(false) {
                std::process::exit(1);
            }
        }
        Commands::Tool { json, cwd, args } => {
            let cwd = cwd.canonicalize()?;
            let Some((requested, tool_args)) = args.split_first() else {
                let response = serde_json::json!({
                    "ok": false,
                    "message": "Tool name required. Allowed tools: kct, kicad-cli, kikit.",
                    "allowed_tools": ["kct", "kicad-cli", "kikit"],
                });
                print_json_or_debug(true, &response)?;
                std::process::exit(1);
            };
            let requested = requested.to_string_lossy();
            let Some(tool) = resolve_tool(&requested) else {
                let response = serde_json::json!({
                    "ok": false,
                    "message": format!("{requested} is not available or is not an allowed KiCad PCB tool."),
                    "allowed_tools": ["kct", "kicad-cli", "kikit"],
                });
                print_json_or_debug(true, &response)?;
                std::process::exit(1);
            };
            let pass_args = if tool_args.is_empty() {
                vec![OsString::from("--help")]
            } else {
                tool_args.to_vec()
            };
            let output = run_command(&tool, &pass_args, &cwd).await?;
            let response = serde_json::json!({
                "ok": output.status == 0,
                "tool": tool,
                "requested_tool": requested,
                "command": std::iter::once(OsString::from(&tool)).chain(pass_args.clone()).map(|item| item.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                "cwd": cwd,
                "stdout": output.stdout,
                "stderr": output.stderr,
            });
            if json {
                print_json_or_debug(true, &response)?;
            } else {
                print!("{}", response["stdout"].as_str().unwrap_or_default());
                eprint!("{}", response["stderr"].as_str().unwrap_or_default());
            }
            if !response["ok"].as_bool().unwrap_or(false) {
                std::process::exit(1);
            }
        }
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

async fn index(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Html<String>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    let warmed = refresh_project_viewer_state(&state, &project).await?;
    Ok(Html(workbench_html(
        &state.cwd,
        &state.session,
        &project,
        &state.projects,
        &warmed,
    )))
}

async fn events_ws(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    Ok(ws.on_upgrade(move |socket| event_socket(state, project, socket)))
}

async fn event_socket(state: AppState, project: ProjectContext, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();

    let initial = match refresh_project_viewer_state(&state, &project).await {
        Ok(warmed) => ServerEvent::Revision {
            project: Some(project.id.clone()),
            revision: warmed.source_revision,
            previous_revision: None,
            warmed_at_ms: warmed.warmed_at_ms,
            reason: "initial".to_string(),
        },
        Err(_) => return,
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
                        if !jobs::event_for_project(&event, &project.id) {
                            continue;
                        }
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
    let project = state
        .projects
        .first()
        .expect("serve initializes at least one project");
    let warmed = match refresh_project_viewer_state(&state, project).await {
        Ok(warmed) => warmed,
        Err(_) => state.warmed.read().await[&project.id].clone(),
    };
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
    let project = state
        .projects
        .first()
        .expect("serve initializes at least one project");
    let warmed = match refresh_project_viewer_state(&state, project).await {
        Ok(warmed) => warmed,
        Err(_) => state.warmed.read().await[&project.id].clone(),
    };
    Json(warmed.manifest)
}

async fn revision(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<RevisionResponse>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    let current = current_source_revision(&project)?;
    let warmed = refresh_project_viewer_state(&state, &project).await?;
    Ok(Json(RevisionResponse {
        ok: true,
        changed: current != warmed.source_revision,
        revision: current,
        warmed_revision: Some(warmed.source_revision),
        warmed_at_ms: Some(warmed.warmed_at_ms),
    }))
}

async fn sources(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<SourcesResponse>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    let warmed = refresh_project_viewer_state(&state, &project).await?;
    Ok(Json(SourcesResponse {
        ok: true,
        revision: warmed.source_revision,
        sources: warmed.sources,
        warmed_at_ms: warmed.warmed_at_ms,
    }))
}

async fn drc(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<CheckResponse>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    Ok(Json(state.builds.check(&project, jobs::Stage::Drc).await?))
}

async fn erc(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<CheckResponse>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    Ok(Json(state.builds.check(&project, jobs::Stage::Erc).await?))
}

async fn quality_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<CheckResponse>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    Ok(Json(
        state.builds.check(&project, jobs::Stage::Quality).await?,
    ))
}

async fn model_glb(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Response, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    let built = state.builds.ensure(&project, jobs::Stage::Glb).await;
    if let Ok((result, dir)) = &built {
        if let Some(name) = result.outputs.iter().find(|name| name.ends_with(".glb")) {
            let bytes = tokio::fs::read(dir.join(name)).await?;
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
    let message = match built {
        Ok((result, _)) => result.message.unwrap_or_else(|| result.log.clone()),
        Err(err) => format!("{err:#}"),
    };
    Ok((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"ok": false, "message": message})),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: String,
    download: Option<String>,
    project: Option<String>,
}

async fn file(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Response, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    let target = safe_rel(&project.root, &query.path)?;
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

async fn libraries_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Html<String>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    Ok(Html(render_library_links(&project)?))
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

async fn refresh_project_viewer_state(
    state: &AppState,
    project: &ProjectContext,
) -> Result<ViewerState> {
    let revision = current_source_revision(project)?;
    {
        let warmed = state.warmed.read().await;
        if let Some(warmed) = warmed.get(&project.id) {
            if warmed.source_revision == revision {
                return Ok(warmed.clone());
            }
        }
    }
    let refreshed = warm_viewer_state(project).await?;
    let mut warmed = state.warmed.write().await;
    warmed.insert(project.id.clone(), refreshed.clone());
    state.builds.request(project);
    Ok(refreshed)
}

async fn warm_viewer_state(project: &ProjectContext) -> Result<ViewerState> {
    let cwd = &project.root;
    let sources = kicad_sources(project)?;
    let source_revision = current_source_revision(project)?;
    let mut manifest = manifest_for(cwd).await?;
    manifest.revision = now_ms().to_string();
    // The GLB is produced by the build pipeline (jobs::Stage::Glb).
    Ok(ViewerState {
        manifest,
        sources,
        source_revision,
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

fn kicad_sources(project: &ProjectContext) -> Result<Vec<SourceFile>> {
    let cwd = &project.root;
    let mut sources = Vec::new();
    for path in revision::walk_scope(project) {
        let path = path.as_path();
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
            let content = std::fs::read_to_string(path).unwrap_or_else(|_| String::new());
            sources.push(SourceFile {
                filename: rel(cwd, path)?,
                content: viewer_source_content(path, content),
            });
        }
    }
    sources.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(sources)
}

fn viewer_source_content(path: &Path, content: String) -> String {
    if path.extension().and_then(|v| v.to_str()) == Some("kicad_pcb") {
        synthesize_kicad10_reference_text(&content)
    } else {
        content
    }
}

fn synthesize_kicad10_reference_text(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        output.push_str(line);
        output.push('\n');
        if !line.trim_start().starts_with("(property \"Reference\" ") {
            continue;
        }

        let Some(reference) = quoted_fields(line).get(1).cloned() else {
            continue;
        };
        let indent = line
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .collect::<String>();
        let mut body = Vec::new();
        let mut depth = paren_delta(line);
        while depth > 0 {
            let Some(next) = lines.next() else { break };
            depth += paren_delta(next);
            output.push_str(next);
            output.push('\n');
            if depth > 0 {
                body.push(next.to_string());
            }
        }
        if body
            .iter()
            .any(|item| item.trim_start().starts_with("(hide"))
        {
            continue;
        }
        output.push_str(&format!(
            "{indent}(fp_text reference \"{}\"\n",
            sexpr_escape(&reference)
        ));
        for item in body {
            output.push_str(&item);
            output.push('\n');
        }
        output.push_str(&format!("{indent})\n"));
    }
    output
}

fn quoted_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if !in_string {
            if ch == '"' {
                in_string = true;
                current.clear();
            }
            continue;
        }
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            fields.push(current.clone());
            in_string = false;
        } else {
            current.push(ch);
        }
    }
    fields
}

fn paren_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == '(' {
            delta += 1;
        } else if ch == ')' {
            delta -= 1;
        }
    }
    delta
}

fn sexpr_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn current_source_revision(project: &ProjectContext) -> Result<String> {
    Ok(revision::project_hashes(project)?.hashes.revision)
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
        ext if is_gerber_extension(ext) || ext == "zip" => "gerber",
        "csv" => match bom::role(path) {
            Some("BOM") => "bom",
            Some(_) => "placement",
            None => "csv",
        },
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
        ".worktrees",
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
        ".worktrees",
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

fn project_contexts(cwd: &Path) -> Result<Vec<ProjectContext>> {
    let config = workspace_config(cwd)?;
    validate_workspace_config(cwd, &config)?;
    let shared_libraries = config.libraries.clone().unwrap_or_default();
    let quality_config = config.quality.clone().unwrap_or_default();
    let workspace_profile = config
        .quality_profile
        .as_ref()
        .map(|path| repo_path(cwd, cwd, path))
        .transpose()?;
    let quality_for =
        |root: &Path, project: Option<&ProjectConfig>| -> Result<quality::QualitySettings> {
            let profile = match project.and_then(|project| project.quality_profile.as_ref()) {
                Some(path) => Some(repo_path(cwd, root, path)?),
                None => workspace_profile.clone(),
            };
            Ok(quality::QualitySettings {
                profile,
                config: quality_config.clone(),
            })
        };
    let Some(projects) = config.projects else {
        return Ok(vec![ProjectContext {
            id: "default".to_string(),
            name: "Default Board".to_string(),
            root: cwd.to_path_buf(),
            config: ProjectConfig {
                id: "default".to_string(),
                name: Some("Default Board".to_string()),
                root: PathBuf::from("."),
                ..ProjectConfig::default()
            },
            shared_libraries,
            repo_root: cwd.to_path_buf(),
            excluded_roots: Vec::new(),
            quality: quality_for(cwd, None)?,
        }]);
    };

    let mut contexts = projects
        .into_iter()
        .map(|project| {
            let root = safe_rel(cwd, &project.root.to_string_lossy())?;
            let name = project.name.clone().unwrap_or_else(|| project.id.clone());
            let quality = quality_for(&root, Some(&project))?;
            Ok(ProjectContext {
                id: project.id.clone(),
                name,
                root,
                config: project,
                shared_libraries: shared_libraries.clone(),
                repo_root: cwd.to_path_buf(),
                excluded_roots: Vec::new(),
                quality,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if contexts.is_empty() {
        contexts.push(ProjectContext {
            id: "default".to_string(),
            name: "Default Board".to_string(),
            root: cwd.to_path_buf(),
            config: ProjectConfig {
                id: "default".to_string(),
                name: Some("Default Board".to_string()),
                root: PathBuf::from("."),
                ..ProjectConfig::default()
            },
            shared_libraries,
            repo_root: cwd.to_path_buf(),
            excluded_roots: Vec::new(),
            quality: quality_for(cwd, None)?,
        });
    }
    // A project whose root contains another project's root (e.g. a root "."
    // board plus boards/*) must not treat the nested board as its source.
    let roots: Vec<PathBuf> = contexts.iter().map(|item| item.root.clone()).collect();
    for context in &mut contexts {
        context.excluded_roots = roots
            .iter()
            .filter(|root| **root != context.root && root.starts_with(&context.root))
            .cloned()
            .collect();
    }

    if let Some(default_project) = config.default_project {
        if let Some(index) = contexts
            .iter()
            .position(|project| project.id == default_project)
        {
            contexts.swap(0, index);
        }
    }
    Ok(contexts)
}

fn validate_workspace_config(cwd: &Path, config: &WorkspaceConfig) -> Result<()> {
    if let Some(version) = config.version {
        if version != 1 {
            return Err(anyhow!(
                "unsupported .kicad-pcb.json version {version}; expected 1"
            ));
        }
    }
    jobs::validate_build_config(&config.build)?;
    if let Some(quality) = &config.quality {
        quality::validate_config(quality)?;
    }
    if let Some(path) = &config.quality_profile {
        validate_profile_path(cwd, cwd, path, "qualityProfile")?;
    }
    let Some(projects) = &config.projects else {
        return Ok(());
    };
    let mut ids = HashSet::new();
    for project in projects {
        if project.id.trim().is_empty() {
            return Err(anyhow!("project id cannot be empty"));
        }
        if !ids.insert(project.id.as_str()) {
            return Err(anyhow!("duplicate project id {:?}", project.id));
        }
        artifacts::validate_artifact_config(project)?;
        let root = safe_rel(cwd, &project.root.to_string_lossy())
            .with_context(|| format!("project {:?} root is invalid", project.id))?;
        if !root.is_dir() {
            return Err(anyhow!(
                "project {:?} root {} is not a directory",
                project.id,
                project.root.display()
            ));
        }
        if let Some(path) = &project.quality_profile {
            validate_profile_path(
                cwd,
                &root,
                path,
                &format!("project {:?} qualityProfile", project.id),
            )?;
        }
        if let Some(kicad) = &project.kicad {
            validate_optional_file(&root, &kicad.project, "kicad.project", "kicad_pro")?;
            validate_optional_file(&root, &kicad.schematic, "kicad.schematic", "kicad_sch")?;
            validate_optional_file(&root, &kicad.pcb, "kicad.pcb", "kicad_pcb")?;
        }
    }
    if let Some(default_project) = &config.default_project {
        if !ids.contains(default_project.as_str()) {
            return Err(anyhow!(
                "defaultProject {:?} does not match any projects[].id",
                default_project
            ));
        }
    }
    Ok(())
}

/// Resolves `path` relative to `base`, requiring the result to stay inside
/// the workspace (so a project may point at a shared profile in the repo).
fn repo_path(repo: &Path, base: &Path, path: &Path) -> Result<PathBuf> {
    let candidate = base
        .join(path)
        .canonicalize()
        .with_context(|| format!("resolve {}", path.display()))?;
    if !candidate.starts_with(repo) {
        return Err(anyhow!("{} escapes the workspace", path.display()));
    }
    Ok(candidate)
}

fn validate_profile_path(repo: &Path, base: &Path, path: &Path, label: &str) -> Result<()> {
    let resolved =
        repo_path(repo, base, path).with_context(|| format!("{label} path is invalid"))?;
    if !resolved.is_file() {
        return Err(anyhow!("{label} {} is not a file", path.display()));
    }
    Ok(())
}

fn validate_optional_file(
    root: &Path,
    path: &Option<PathBuf>,
    label: &str,
    extension: &str,
) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    let resolved = safe_rel(root, &path.to_string_lossy())
        .with_context(|| format!("{label} path is invalid"))?;
    if !resolved.is_file() {
        return Err(anyhow!("{label} {} is not a file", path.display()));
    }
    if resolved.extension().and_then(|value| value.to_str()) != Some(extension) {
        return Err(anyhow!(
            "{label} {} must have .{extension} extension",
            path.display()
        ));
    }
    Ok(())
}

fn workspace_config(cwd: &Path) -> Result<WorkspaceConfig> {
    let path = cwd.join(".kicad-pcb.json");
    if !path.exists() {
        return Ok(WorkspaceConfig {
            version: None,
            default_project: None,
            libraries: None,
            projects: None,
            build: None,
            quality_profile: None,
            quality: None,
        });
    }
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))
}

fn selected_project(state: &AppState, id: Option<&str>) -> Result<ProjectContext> {
    find_project(&state.projects, id)
}

fn find_project(projects: &[ProjectContext], id: Option<&str>) -> Result<ProjectContext> {
    let wanted = id.filter(|id| !id.is_empty());
    if let Some(wanted) = wanted {
        if let Some(project) = projects.iter().find(|project| project.id == wanted) {
            return Ok(project.clone());
        }
        return Err(anyhow!("unknown KiCad PCB project {wanted:?}"));
    }
    projects
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no KiCad PCB projects configured"))
}

fn cli_project_context(cwd: &Path, id: Option<&str>) -> Result<ProjectContext> {
    find_project(&project_contexts(cwd)?, id)
}

fn project_summary(project: &ProjectContext) -> serde_json::Value {
    serde_json::json!({
        "id": project.id,
        "name": project.name,
        "root": project.root,
        "kicad": project.config.kicad,
        "artifacts": project.config.artifacts,
        "libraries": {
            "inherited": project.config.inherit_libraries.unwrap_or(true),
            "shared": project.shared_libraries,
            "project": project.config.libraries,
        },
        "manufacturer": project.config.manufacturer,
        "quality_profile": project.quality.profile,
    })
}

fn library_warnings(repo_root: &Path, project: &ProjectContext) -> Vec<String> {
    let mut warnings = Vec::new();
    if project.config.inherit_libraries.unwrap_or(true) {
        warnings.extend(missing_library_warnings(
            repo_root,
            &project.shared_libraries,
            "shared",
        ));
    }
    if let Some(libraries) = &project.config.libraries {
        warnings.extend(missing_library_warnings(
            &project.root,
            libraries,
            &format!("project {}", project.id),
        ));
    }
    warnings
}

fn missing_library_warnings(root: &Path, libraries: &LibraryConfig, label: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    for (kind, paths) in [
        ("symbols", libraries.symbols.as_ref()),
        ("footprints", libraries.footprints.as_ref()),
        ("models", libraries.models.as_ref()),
        ("datasheets", libraries.datasheets.as_ref()),
    ] {
        let Some(paths) = paths else { continue };
        for path in paths {
            let target = root.join(path);
            if !target.exists() {
                warnings.push(format!(
                    "{label} {kind} library path does not exist: {}",
                    path.display()
                ));
            }
        }
    }
    warnings
}

#[derive(Debug, Clone, Default)]
struct SchematicSymbolLink {
    reference: String,
    value: Option<String>,
    symbol: String,
    footprint: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct PcbFootprintLink {
    reference: String,
    footprint: String,
    models: Vec<String>,
}

#[derive(Debug, Clone)]
struct LibraryLinkRow {
    reference: String,
    value: String,
    symbol: String,
    schematic_footprint: String,
    pcb_footprint: String,
    models: Vec<String>,
}

fn render_library_links(project: &ProjectContext) -> Result<String> {
    let rows = library_links(project)?;
    if rows.is_empty() {
        return Ok(
            "<p class='muted'>No schematic symbol to footprint links found.</p>".to_string(),
        );
    }
    let body = rows
        .iter()
        .map(|row| {
            let models = if row.models.is_empty() {
                "<span class='muted'>No 3D model linked from footprint</span>".to_string()
            } else {
                row.models
                    .iter()
                    .map(|model| format!("<code>{}</code>", escape(model)))
                    .collect::<Vec<_>>()
                    .join("<br>")
            };
            format!(
                "<tr><td><strong>{}</strong><div class='muted'>{}</div></td><td><code>{}</code></td><td><code>{}</code></td><td><code>{}</code></td><td>{}</td></tr>",
                escape(&row.reference),
                escape(&row.value),
                escape(&row.symbol),
                escape(&row.schematic_footprint),
                escape(&row.pcb_footprint),
                models,
            )
        })
        .collect::<String>();
    Ok(format!(
        "<table class='library-link-table'><thead><tr><th>Ref / value</th><th>Symbol</th><th>Schematic footprint</th><th>PCB footprint</th><th>3D model</th></tr></thead><tbody>{body}</tbody></table>"
    ))
}

fn library_links(project: &ProjectContext) -> Result<Vec<LibraryLinkRow>> {
    let schematic_symbols = pick_project_file(project, "kicad_sch")?
        .map(|path| parse_schematic_symbols(&path))
        .transpose()?
        .unwrap_or_default();
    let pcb_footprints = pick_project_file(project, "kicad_pcb")?
        .map(|path| parse_pcb_footprints(&path))
        .transpose()?
        .unwrap_or_default();
    let footprints_by_ref = pcb_footprints
        .into_iter()
        .filter(|footprint| !footprint.reference.is_empty())
        .map(|footprint| (footprint.reference.clone(), footprint))
        .collect::<HashMap<_, _>>();

    let mut rows = schematic_symbols
        .into_iter()
        .filter(|symbol| !symbol.reference.is_empty())
        .map(|symbol| {
            let pcb = footprints_by_ref.get(&symbol.reference);
            LibraryLinkRow {
                reference: symbol.reference,
                value: symbol.value.unwrap_or_else(|| "-".to_string()),
                symbol: symbol.symbol,
                schematic_footprint: symbol.footprint.unwrap_or_else(|| "-".to_string()),
                pcb_footprint: pcb
                    .map(|footprint| footprint.footprint.clone())
                    .unwrap_or_else(|| "-".to_string()),
                models: pcb
                    .map(|footprint| footprint.models.clone())
                    .unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| natural_ref_key(&row.reference));
    Ok(rows)
}

fn parse_schematic_symbols(path: &Path) -> Result<Vec<SchematicSymbolLink>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read schematic {}", path.display()))?;
    Ok(sexpr_blocks(&content, "symbol")
        .into_iter()
        .filter_map(|block| {
            let symbol = sexpr_lib_id(&block)?;
            let reference = sexpr_property(&block, "Reference")?;
            Some(SchematicSymbolLink {
                reference,
                value: sexpr_property(&block, "Value"),
                symbol,
                footprint: sexpr_property(&block, "Footprint"),
            })
        })
        .filter(|symbol| {
            !symbol.reference.starts_with('#')
                && !symbol.reference.starts_with("Sheet")
                && symbol.reference != "REF**"
        })
        .collect())
}

fn parse_pcb_footprints(path: &Path) -> Result<Vec<PcbFootprintLink>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read PCB {}", path.display()))?;
    Ok(sexpr_blocks(&content, "footprint")
        .into_iter()
        .filter_map(|block| {
            let footprint = first_quoted(&block)?;
            let reference = sexpr_property(&block, "Reference")
                .or_else(|| legacy_fp_text(&block, "reference"))
                .unwrap_or_default();
            let models = sexpr_model_paths(&block);
            Some(PcbFootprintLink {
                reference,
                footprint,
                models,
            })
        })
        .filter(|footprint| !footprint.reference.is_empty() && footprint.reference != "REF**")
        .collect())
}

fn sexpr_blocks(content: &str, head: &str) -> Vec<String> {
    let needle = format!("({head}");
    let mut blocks = Vec::new();
    let mut offset = 0;
    while let Some(found) = content[offset..].find(&needle) {
        let start = offset + found;
        if !is_sexpr_head_boundary(content, start + needle.len()) {
            offset = start + needle.len();
            continue;
        }
        let Some(end) = sexpr_block_end(content, start) else {
            break;
        };
        blocks.push(content[start..end].to_string());
        offset = start + 1;
    }
    blocks
}

fn is_sexpr_head_boundary(content: &str, index: usize) -> bool {
    content[index..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_whitespace() || ch == ')' || ch == '"')
}

fn sexpr_block_end(content: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (relative, ch) in content[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + relative + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn first_quoted(block: &str) -> Option<String> {
    quoted_fields(block.lines().next().unwrap_or_default())
        .into_iter()
        .next()
}

fn sexpr_lib_id(block: &str) -> Option<String> {
    let index = block.find("(lib_id ")?;
    quoted_fields(&block[index..]).into_iter().next()
}

fn sexpr_property(block: &str, name: &str) -> Option<String> {
    let mut offset = 0;
    while let Some(found) = block[offset..].find("(property ") {
        let start = offset + found;
        let fields = quoted_fields(&block[start..]);
        if fields.first().is_some_and(|field| field == name) {
            return fields.get(1).cloned();
        }
        offset = start + "(property ".len();
    }
    None
}

fn legacy_fp_text(block: &str, kind: &str) -> Option<String> {
    let needle = format!("(fp_text {kind} ");
    let index = block.find(&needle)?;
    quoted_fields(&block[index..]).into_iter().next()
}

fn sexpr_model_paths(block: &str) -> Vec<String> {
    let mut models = sexpr_blocks(block, "model")
        .into_iter()
        .filter_map(|model| first_quoted(&model))
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

fn natural_ref_key(reference: &str) -> (String, u32, String) {
    let prefix = reference
        .chars()
        .take_while(|ch| !ch.is_ascii_digit())
        .collect::<String>();
    let digits = reference
        .chars()
        .skip(prefix.len())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    let number = digits.parse::<u32>().unwrap_or(u32::MAX);
    (prefix, number, reference.to_string())
}

fn configured_source(project: &ProjectContext, extension: &str) -> Result<Option<PathBuf>> {
    let Some(kicad) = &project.config.kicad else {
        return Ok(None);
    };
    let configured = match extension {
        "kicad_pro" => &kicad.project,
        "kicad_sch" => &kicad.schematic,
        "kicad_pcb" => &kicad.pcb,
        _ => &None,
    };
    let Some(path) = configured else {
        return Ok(None);
    };
    Ok(Some(safe_rel(&project.root, &path.to_string_lossy())?))
}

fn pick_project_file(project: &ProjectContext, extension: &str) -> Result<Option<PathBuf>> {
    if let Some(path) = configured_source(project, extension)? {
        return Ok(Some(path));
    }
    pick_one(&project.root, extension)
}

fn export_out(project: &ProjectContext, out: &Path, kind: &str) -> PathBuf {
    if out.as_os_str() != "." {
        return out.to_path_buf();
    }
    let artifacts = project.config.artifacts.as_ref();
    let configured = match kind {
        "gerbers" => artifacts.and_then(|item| item.gerbers.as_ref()),
        "jlcpcb" => artifacts.and_then(|item| item.jlcpcb.as_ref()),
        "checks" => artifacts.and_then(|item| item.checks.as_ref()),
        "docs" => artifacts.and_then(|item| item.docs.as_ref()),
        _ => artifacts.and_then(|item| item.fab.as_ref()),
    };
    configured
        .map(|path| project.root.join(path))
        .unwrap_or_else(|| project.root.join("fab").join(kind))
}

async fn run_check(project: &ProjectContext, kind: &str) -> Result<CheckResponse> {
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
        pick_project_file(project, "kicad_pcb")?
    } else {
        pick_project_file(project, "kicad_sch")?
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
    let output = run_command(&cli, &args, &project.root).await?;
    let report_text = tokio::fs::read_to_string(&report).await.unwrap_or_default();
    Ok(CheckResponse {
        ok: output.status == 0,
        source: Some(rel(&project.root, &source)?),
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

async fn export_glb(project: &ProjectContext, out: &Path) -> Result<serde_json::Value> {
    let Some(cli) = kicad_cli() else {
        return Ok(serde_json::json!({"ok": false, "message": "kicad-cli is required"}));
    };
    let Some(board) = pick_project_file(project, "kicad_pcb")? else {
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
    let output = run_command(&cli, &args, &project.root).await?;
    Ok(serde_json::json!({
        "ok": output.status == 0,
        "project": project.id,
        "board": rel(&project.root, &board)?,
        "out": out,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

async fn export_fab(
    project: &ProjectContext,
    out: &Path,
    jlcpcb: bool,
) -> Result<serde_json::Value> {
    let Some(cli) = kicad_cli() else {
        return Ok(serde_json::json!({"ok": false, "message": "kicad-cli is required"}));
    };
    let Some(board) = pick_project_file(project, "kicad_pcb")? else {
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
        &project.root,
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
        &project.root,
    )
    .await?;
    let mut status = gerber.status | drill.status;
    let mut stdout = format!("{}{}", gerber.stdout, drill.stdout);
    let mut stderr = format!("{}{}", gerber.stderr, drill.stderr);
    if jlcpcb {
        if let Some(schematic) = pick_project_file(project, "kicad_sch")? {
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
                &project.root,
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
                &project.root,
            )
            .await?;
            status |= pos.status;
            stdout.push_str(&pos.stdout);
            stderr.push_str(&pos.stderr);
            if raw.exists() {
                convert_position_csv(&raw, &cpl)?;
                if bom.exists() {
                    if let Some(count) = artifacts::apply_placement_offsets(project, &bom, &cpl)? {
                        stdout.push_str(&format!(
                            "Applied JLCPCB placement offsets to {count} placements\n"
                        ));
                    }
                }
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
        "project": project.id,
        "board": rel(&project.root, &board)?,
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
    jobs::run_command(program, args, cwd).await
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

async fn kct_version() -> Option<String> {
    let tool = kct_cli()?;
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
        .or_else(|| std::env::var("KICAD_CLI").ok())
        .or_else(managed_kicad_cli)
        .or_else(|| find_on_path("kicad-cli"))
}

/// KiCad AppImage runtime installed by `setup --install-kicad` (Python
/// installer) under `.runtime/kicad/current`.
fn managed_kicad_cli() -> Option<String> {
    let current = plugin_dir()
        .ok()?
        .join(".runtime")
        .join("kicad")
        .join("current");
    [
        current.join("squashfs-root/usr/bin/kicad-cli"),
        current.join("usr/bin/kicad-cli"),
        current.join("kicad-cli"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .map(|candidate| candidate.display().to_string())
}

fn kct_cli() -> Option<String> {
    std::env::var("KICAD_PCB_KCT")
        .ok()
        .or_else(|| managed_kct_cli().ok().flatten())
        .or_else(|| find_on_path("kct"))
        .or_else(|| find_on_path("kicad-tools"))
}

fn resolve_tool(name: &str) -> Option<String> {
    match name {
        "kct" | "kicad-tools" => kct_cli(),
        "kicad-cli" => kicad_cli(),
        "kikit" => find_on_path("kikit"),
        _ => None,
    }
}

fn managed_kct_cli() -> Result<Option<String>> {
    let bin = if cfg!(windows) {
        "Scripts/kct.exe"
    } else {
        "bin/kct"
    };
    let candidate = plugin_dir()?.join(".runtime").join("kicad-tools").join(bin);
    Ok(candidate.exists().then(|| candidate.display().to_string()))
}

async fn install_kicad_tools_runtime(force: bool, extra: &str) -> Result<serde_json::Value> {
    let Some(python) = find_on_path("python3").or_else(|| find_on_path("python")) else {
        return Ok(serde_json::json!({
            "ok": false,
            "message": "Python 3 is required to install kicad-tools.",
        }));
    };
    let runtime = plugin_dir()?.join(".runtime").join("kicad-tools");
    if force && runtime.exists() {
        let backup = runtime.with_file_name(format!("kicad-tools.old-{}", now_ms()));
        std::fs::rename(&runtime, backup)?;
    }
    if !runtime.exists() {
        let output = run_command(
            &python,
            &["-m".into(), "venv".into(), runtime.as_os_str().into()],
            &plugin_dir()?,
        )
        .await?;
        if output.status != 0 {
            return Ok(serde_json::json!({
                "ok": false,
                "message": "Failed to create kicad-tools virtualenv.",
                "stdout": output.stdout,
                "stderr": output.stderr,
            }));
        }
    }
    let pip = runtime
        .join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(if cfg!(windows) { "pip.exe" } else { "pip" });
    let package = kicad_tools_package(extra);
    let mut logs = Vec::new();
    for args in [
        vec!["install".into(), "--upgrade".into(), "pip".into()],
        vec!["install".into(), "--upgrade".into(), package.clone().into()],
    ] {
        let output = run_command(&pip.display().to_string(), &args, &plugin_dir()?).await?;
        let log = serde_json::json!({
            "command": std::iter::once(pip.display().to_string()).chain(args.iter().map(|item| item.to_string_lossy().into_owned())).collect::<Vec<_>>(),
            "code": output.status,
            "stdout": output.stdout.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>(),
            "stderr": output.stderr.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>(),
        });
        let ok = output.status == 0;
        logs.push(log);
        if !ok {
            return Ok(serde_json::json!({
                "ok": false,
                "message": format!("Failed to install {package}."),
                "logs": logs,
            }));
        }
    }
    Ok(serde_json::json!({
        "ok": kct_cli().is_some(),
        "package": package,
        "kct": kct_cli(),
        "version": kct_version().await,
        "runtime_dir": runtime,
        "logs": logs,
    }))
}

fn kicad_tools_package(extra: &str) -> String {
    if matches!(extra, "" | "base" | "none") {
        "kicad-tools".to_string()
    } else if extra == "agent" {
        "kicad-tools[placement,parts,datasheet,report,native]".to_string()
    } else {
        format!("kicad-tools[{extra}]")
    }
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
    path.extension()
        .and_then(|v| v.to_str())
        .is_some_and(is_gerber_extension)
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

fn workbench_html(
    cwd: &Path,
    session: &str,
    project: &ProjectContext,
    projects: &[ProjectContext],
    warmed: &ViewerState,
) -> String {
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
    let project_id = serde_json::to_string(&project.id).unwrap_or_else(|_| "\"\"".to_string());
    let project_options = projects
        .iter()
        .map(|item| {
            let selected = if item.id == project.id {
                " selected"
            } else {
                ""
            };
            format!(
                "<option value='{}'{selected}>{}</option>",
                escape_attr(&item.id),
                escape(&item.name)
            )
        })
        .collect::<String>();
    format!(
        r##"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>KiCad PCB · Agent Portal</title>
<style>
body{{margin:0;background:#16161e;color:#c0caf5;font-family:Inter,ui-sans-serif,system-ui,sans-serif}}header{{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:14px 16px;border-bottom:1px solid #3b4261;background:#1a1b26}}h1{{margin:0;font-size:18px;color:#e6e9f5}}a{{color:#7dcfff}}select{{border:1px solid #3b4261;border-radius:6px;background:#24283b;color:#c0caf5;padding:7px 10px}}.muted{{color:#9aa5ce}}.ok{{color:#9ece6a}}.tabs{{display:flex;gap:4px;flex-wrap:wrap;padding:8px;background:#1f2335;position:sticky;top:0;z-index:2}}.tabs button{{border:1px solid #3b4261;color:#c0caf5;background:#24283b;padding:7px 10px;border-radius:6px;cursor:pointer}}.tabs button.active{{background:#7aa2f7;color:#10131d;border-color:#7aa2f7}}main{{padding:14px}}section{{display:none;min-height:55vh}}section.active{{display:block}}.card{{border:1px solid #3b4261;border-radius:8px;background:#1f2335;padding:12px}}iframe{{width:100%;height:70vh;border:1px solid #3b4261;border-radius:8px;background:#11131d}}pre{{white-space:pre-wrap;overflow:auto;background:#11131d;padding:12px;border-radius:6px}}code{{color:#7dcfff}}.check-grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:12px}}.check-summary{{display:flex;flex-wrap:wrap;gap:8px;margin:8px 0 12px}}.pill{{border:1px solid #3b4261;border-radius:999px;padding:4px 8px;background:#24283b;font-size:12px}}.pill.error{{border-color:#f7768e;color:#f7768e}}.pill.warning{{border-color:#e0af68;color:#e0af68}}.pill.ok{{border-color:#9ece6a;color:#9ece6a}}.issue-list{{display:grid;gap:8px}}.issue{{border:1px solid #3b4261;border-radius:6px;background:#181b29;padding:10px}}.issue-head{{display:flex;gap:8px;align-items:center;justify-content:space-between}}.severity{{text-transform:uppercase;font-size:11px;letter-spacing:.04em;border-radius:4px;padding:2px 6px}}.severity.error{{background:#f7768e22;color:#f7768e}}.severity.warning{{background:#e0af6822;color:#e0af68}}.severity.info{{background:#7dcfff22;color:#7dcfff}}.quality-card{{grid-column:1/-1}}details.quality-rule{{border:1px solid #3b4261;border-radius:6px;background:#181b29;padding:8px 10px;margin:6px 0}}details.quality-rule summary{{cursor:pointer;display:flex;gap:8px;align-items:center;flex-wrap:wrap}}details.quality-rule .issue-list{{margin-top:8px}}.items{{margin:8px 0 0;padding-left:18px}}details.raw{{margin-top:12px}}details.raw summary{{cursor:pointer;color:#7dcfff}}.gerber-layout{{display:grid;grid-template-columns:minmax(0,1fr) 280px;gap:12px}}#gerberViewer{{height:70vh;min-height:420px;border:1px solid #3b4261;border-radius:8px;overflow:hidden;background:#11131d}}.side-panel{{border:1px solid #3b4261;border-radius:8px;background:#16161e;padding:10px;overflow:auto;max-height:70vh}}.side-panel h3{{font-size:13px;margin:0 0 8px;color:#e6e9f5}}.side-panel ul{{margin:0 0 14px;padding-left:18px}}.warning{{color:#e0af68}}@media (max-width: 860px){{header{{align-items:flex-start;flex-direction:column}}.gerber-layout{{grid-template-columns:1fr}}.side-panel{{max-height:none}}}}
</style></head><body>
<header><div><h1>KiCad PCB Workbench</h1><div class="muted">Session {session} · {cwd}</div></div><label class="muted">Board <select id="projectSelect">{project_options}</select></label></header>
<div id="buildStrip" data-project="{project_attr}"></div>
<nav class="tabs">{tabs}</nav><main>
<section id="schematic"><div class="card"><h2>Schematic</h2><iframe class="native-viewer" data-kind="schematic" src="/kicad-viewer/runtime.html"></iframe></div></section>
<section id="pcb"><div class="card"><h2>PCB</h2><iframe class="native-viewer" data-kind="pcb" src="/kicad-viewer/runtime.html"></iframe></div></section>
<section id="3d"><div class="card"><h2>3D Board</h2><iframe class="model-viewer" data-kind="model" src="/kicad-viewer/runtime.html"></iframe></div></section>
<section id="checks"><div class="check-grid"><div class="card"><h2>DRC</h2><div id="drcOut">Open checks...</div></div><div class="card"><h2>ERC</h2><div id="ercOut">Open checks...</div></div><div class="card quality-card"><h2>Layout quality</h2><div id="qualityOut">Open checks...</div></div></div></section>
<section id="gerbers"><div class="card"><h2>Gerbers</h2><div class="gerber-layout"><div id="gerberViewer"></div><aside class="side-panel"><h3>Layers</h3><div id="gerberStatus" class="muted">Loading Gerber viewer...</div><ul id="gerberLayers"></ul><h3>Files</h3><div id="gerberOrigin" class="muted"></div><ul id="gerberFiles"></ul><div id="gerberWarnings"></div><div id="gerberSkipped" class="muted"></div></aside></div></div></section>
<section id="step"><div class="card"><h2>STEP</h2><ul>{files}</ul></div></section>
<section id="bom"><div class="card"><h2>BOM / Assembly CSV</h2><div id="bomContent">Select this tab to load BOM/assembly artifacts.</div></div></section>
<section id="libraries"><div class="card"><h2>Libraries</h2><div id="librariesContent">Select this tab to load symbol, footprint, and 3D-model links.</div></div></section>
<section id="analysis"><div class="card"><h2>Analysis</h2><p class="muted">Analysis workflow is pending the Rust port.</p></div></section>
<section id="panelization"><div class="card"><h2>Panelization</h2><p class="muted">Panelization workflow is pending the Rust port.</p></div></section>
</main><script>
let sourceSnapshot;
let sourceSnapshotPromise;
let activeTab = "schematic";
let refreshInFlight = false;
let gerberViewer;
let gerberLoadPromise;
let gerberRevisionKey;
let gerberSources = [];
let gerberSourceKey;
const projectId = {project_id};
const projectParam = () => `project=${{encodeURIComponent(projectId)}}`;
const api = path => `${{path}}${{path.includes("?") ? "&" : "?"}}${{projectParam()}}`;
const loadSources = async () => {{
  if (sourceSnapshot) return sourceSnapshot;
  sourceSnapshotPromise ||= fetch(api("/api/kicad/sources"))
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
const modelUrl = revision => `/api/kicad/model.glb?${{projectParam()}}&rev=${{encodeURIComponent(revision || "current")}}`;
const viewerFrames = () => document.querySelectorAll("iframe.native-viewer, iframe.model-viewer");
const postSnapshot = async frame => {{
  if (frame.dataset.kind === "model") {{
    const payload = await loadSources();
    frame.contentWindow?.postMessage({{type:"kicad-pcb-snapshot", kind:"model", url:modelUrl(payload.revision), active:true}}, location.origin);
    return;
  }}
  const payload = await loadSources();
  frame.contentWindow?.postMessage({{type:"kicad-pcb-snapshot", kind:"native", context:frame.dataset.kind, revision:payload.revision, sources:payload.sources, active:true}}, location.origin);
}};
window.addEventListener("message", event => {{
  if (event.origin !== location.origin) return;
  if (event.data?.type === "kicad-pcb-runtime-ready") {{
    const frame = [...viewerFrames()].find(item => item.contentWindow === event.source);
    if (frame) void postSnapshot(frame);
  }}
}});
const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({{"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}}[c]));
const parseReport = check => {{
  try {{ return JSON.parse(check?.report || "{{}}"); }} catch {{ return {{}}; }}
}};
const reportIssues = (kind, report) => {{
  if (kind === "erc") {{
    return (report.sheets || []).flatMap(sheet => (sheet.violations || []).map(issue => ({{...issue, sheet:sheet.path}})));
  }}
  return [
    ...(report.unconnected_items || []).map(issue => ({{...issue, type:issue.type || "unconnected_items"}})),
    ...(report.schematic_parity || []).map(issue => ({{...issue, type:issue.type || "schematic_parity"}})),
    ...(report.violations || []),
  ];
}};
const issueCard = issue => {{
  const severity = issue.severity || "warning";
  const itemList = (issue.items || []).map(item => {{
    const pos = item.pos ? ` <span class="muted">(${{esc(item.pos.x)}}, ${{esc(item.pos.y)}})</span>` : "";
    return `<li>${{esc(item.description || item.uuid || "item")}}${{pos}}</li>`;
  }}).join("");
  const meta = [issue.type, issue.sheet ? `sheet ${{issue.sheet}}` : ""].filter(Boolean).join(" · ");
  return `<article class="issue"><div class="issue-head"><strong>${{esc(issue.description || issue.type || "Issue")}}</strong><span class="severity ${{esc(severity)}}">${{esc(severity)}}</span></div><div class="muted">${{esc(meta)}}</div>${{itemList ? `<ul class="items">${{itemList}}</ul>` : ""}}</article>`;
}};
const renderCheck = (target, check, kind) => {{
  const report = parseReport(check);
  const issues = reportIssues(kind, report);
  const counts = issues.reduce((acc, issue) => {{
    const severity = issue.severity || "warning";
    acc[severity] = (acc[severity] || 0) + 1;
    return acc;
  }}, {{}});
  const status = check.ok ? "ok" : (counts.error || issues.length ? "error" : "warning");
  const summary = (check.stdout || check.message || "").split("\\n").filter(Boolean).slice(0, 3).join(" · ");
  target.innerHTML = `<div class="check-summary"><span class="pill ${{status}}">${{check.ok ? "clean" : "needs attention"}}</span><span class="pill">${{esc(report.kicad_version || check.source || "KiCad")}}</span><span class="pill">${{issues.length}} issue${{issues.length === 1 ? "" : "s"}}</span><span class="pill error">${{counts.error || 0}} errors</span><span class="pill warning">${{counts.warning || 0}} warnings</span></div><p class="muted">${{esc(summary)}}</p>${{issues.length ? `<div class="issue-list">${{issues.map(issueCard).join("")}}</div>` : "<p class='ok'>No issues reported.</p>"}}<details class="raw"><summary>Raw report</summary><pre>${{esc(JSON.stringify(check, null, 2))}}</pre></details>`;
}};
const renderQuality = (target, check) => {{
  const report = parseReport(check);
  const issues = report.violations || [];
  const summary = report.summary || {{}};
  const groups = new Map();
  for (const issue of issues) {{
    if (!groups.has(issue.type)) groups.set(issue.type, []);
    groups.get(issue.type).push(issue);
  }}
  const status = summary.errors ? "error" : (summary.warnings ? "warning" : "ok");
  const profile = report.profile?.path ? `profile ${{report.profile.name || report.profile.path}}` : "no qualityProfile";
  const kct = report.kct?.version || "kct missing";
  const superseded = Object.entries(report.audits?.superseded_by_kct || {{}}).map(([id, rule]) => `${{id}} -> kct ${{rule}}`);
  const notes = [...(report.notes || []), ...(superseded.length ? [`Retired audits (kct covers them): ${{superseded.join(", ")}}`] : [])];
  const excluded = Object.values(report.kct?.excluded_drc_duplicates || {{}}).reduce((a, b) => a + b, 0);
  const sevRank = {{error:0, warning:1, info:2}};
  const rows = [...groups.entries()].sort((a, b) => (sevRank[a[1][0].severity] ?? 3) - (sevRank[b[1][0].severity] ?? 3) || b[1].length - a[1].length).map(([rule, list]) => {{
    const first = list[0];
    const meta = [first.source, first.profile_item ? `profile ${{first.profile_item}}` : ""].filter(Boolean).join(" · ");
    const shown = list.slice(0, 50).map(issueCard).join("");
    const more = list.length > 50 ? `<p class="muted">${{list.length - 50}} more in the raw report</p>` : "";
    return `<details class="quality-rule"><summary><span class="severity ${{esc(first.severity)}}">${{esc(first.severity)}}</span> <strong>${{esc(rule)}}</strong> <span class="pill">${{list.length}}</span> <span class="muted">${{esc(meta)}}</span></summary><div class="issue-list">${{shown}}${{more}}</div></details>`;
  }}).join("");
  const message = check.message ? `<p class="warning">${{esc(check.message)}}</p>` : "";
  target.innerHTML = `<div class="check-summary"><span class="pill ${{status}}">${{status === "ok" ? "clean" : "needs attention"}}</span><span class="pill">${{esc(profile)}}</span><span class="pill">${{esc(kct)}}</span><span class="pill error">${{summary.errors || 0}} errors</span><span class="pill warning">${{summary.warnings || 0}} warnings</span><span class="pill">${{summary.infos || 0}} info</span></div>${{message}}${{notes.map(note => `<p class="muted">${{esc(note)}}</p>`).join("")}}${{excluded ? `<p class="muted">${{excluded}} kct findings duplicating native DRC hidden (quality.includeDrcRules).</p>` : ""}}${{rows || (check.report ? "<p class='ok'>No layout-quality findings.</p>" : "")}}<details class="raw"><summary>Raw report</summary><pre>${{esc(JSON.stringify(report.summary ? report : check, null, 2))}}</pre></details>`;
}};
const loadChecks = async () => {{
  const json = r => r.json().catch(() => ({{ok:false, message:`HTTP ${{r.status}}`}}));
  const quality = fetch(api("/api/kicad/quality")).then(json).then(check => renderQuality(document.getElementById("qualityOut"), check), err => {{ document.getElementById("qualityOut").textContent = String(err); }});
  const [drc, erc] = await Promise.all([fetch(api("/api/kicad/drc")).then(r=>r.json()), fetch(api("/api/kicad/erc")).then(r=>r.json())]);
  renderCheck(document.getElementById("drcOut"), drc, "drc");
  renderCheck(document.getElementById("ercOut"), erc, "erc");
  await quality;
}};
const renderGerberProject = project => {{
  const layers = document.getElementById("gerberLayers");
  const warnings = document.getElementById("gerberWarnings");
  const skipped = document.getElementById("gerberSkipped");
  const status = document.getElementById("gerberStatus");
  layers.innerHTML = "";
  for (const layer of project.layers || []) {{
    const item = document.createElement("li");
    item.textContent = `${{layer.label}} · ${{layer.name}}`;
    layers.appendChild(item);
  }}
  warnings.innerHTML = "";
  for (const warning of project.warnings || []) {{
    const item = document.createElement("div");
    item.className = "warning";
    item.textContent = warning;
    warnings.appendChild(item);
  }}
  skipped.innerHTML = "";
  if ((project.skipped || []).length) {{
    const title = document.createElement("h3");
    title.textContent = "Skipped";
    skipped.appendChild(title);
  }}
  for (const itemText of project.skipped || []) {{
    const item = document.createElement("div");
    item.textContent = itemText;
    skipped.appendChild(item);
  }}
  status.textContent = `${{(project.layers || []).length}} layer${{(project.layers || []).length === 1 ? "" : "s"}} loaded`;
}};
// Gerber sources: the current cached build when ready, else published files.
const fetchGerberSources = async () => {{
  const payload = await fetch(api("/api/kicad/gerbers")).then(r => r.json());
  gerberSources = payload.files || [];
  document.getElementById("gerberOrigin").textContent = payload.label || "";
  document.getElementById("gerberFiles").innerHTML = gerberSources.length
    ? gerberSources.map(item => `<li><a href="${{esc(item.url)}}&download=1">${{esc(item.path)}}</a></li>`).join("")
    : "<li class='muted'>No Gerber or drill files yet.</li>";
  return `${{payload.origin}}-${{payload.stage || ""}}-${{payload.revision || ""}}`;
}};
const gerberSourcePayload = revision => gerberSources.map(item => ({{name:item.path.split("/").pop(), url:`${{item.url}}&rev=${{encodeURIComponent(revision || "current")}}`}}));
const loadGerbers = async () => {{
  if (gerberViewer) return gerberViewer;
  if (gerberLoadPromise) return gerberLoadPromise;
  gerberLoadPromise = (async () => {{
    const status = document.getElementById("gerberStatus");
    const sourceKey = await fetchGerberSources();
    if (!gerberSources.length) {{
      status.textContent = "No generated Gerber or drill files found.";
      gerberLoadPromise = undefined;
      return undefined;
    }}
    const module = await import("/kicad-viewer/gerber-view/gerber_view.js");
    await module.default("/kicad-viewer/gerber-view/gerber_view_bg.wasm");
    const viewer = new module.GerberViewer({{controls:true, background:"#11131d", padding:18}});
    viewer.mount(document.getElementById("gerberViewer"));
    viewer.onChange(renderGerberProject);
    const project = await viewer.setSources(gerberSourcePayload(sourceKey));
    gerberRevisionKey = sourceKey;
    renderGerberProject(project);
    viewer.fit();
    gerberViewer = viewer;
    return viewer;
  }})().catch(err => {{
    document.getElementById("gerberStatus").textContent = `Gerber viewer failed: ${{err?.message || err}}`;
    console.error("Gerber viewer failed", err);
    gerberLoadPromise = undefined;
  }});
  return gerberLoadPromise;
}};
const refreshGerbers = async () => {{
  if (!gerberViewer) {{ if (activeTab === "gerbers") await loadGerbers(); return; }}
  const nextRevision = await fetchGerberSources();
  if (gerberRevisionKey === nextRevision || !gerberSources.length) return;
  gerberRevisionKey = nextRevision;
  const project = await gerberViewer.setSources(gerberSourcePayload(nextRevision));
  renderGerberProject(project);
  gerberViewer.resize();
}};
const loadBom = async () => {{
  const el=document.getElementById("bomContent");
  try {{ const r=await fetch(api("/api/kicad/bom")); if(!r.ok) throw new Error(`HTTP ${{r.status}}`); el.innerHTML=await r.text(); }}
  catch(e) {{ el.textContent=`Unable to load BOM: ${{e.message}}`; }}
}};
const loadLibraries = async () => {{
  const el=document.getElementById("librariesContent");
  try {{ const r=await fetch(api("/api/kicad/libraries")); if(!r.ok) throw new Error(`HTTP ${{r.status}}`); el.innerHTML=await r.text(); }}
  catch(e) {{ el.textContent=`Unable to load libraries: ${{e.message}}`; }}
}};
const tableStyle=document.createElement("style");tableStyle.textContent=".bom-table,.library-link-table{{border-collapse:collapse;width:100%;font-size:13px}}.bom-table th,.bom-table td,.library-link-table th,.library-link-table td{{padding:8px;text-align:left;border-bottom:1px solid #565f89;white-space:pre-wrap;vertical-align:top}}.bom-table th,.library-link-table th{{position:sticky;top:0;background:#24283b}}";document.head.append(tableStyle);
const refreshPane = async () => {{
  if (refreshInFlight) return;
  refreshInFlight = true;
  try {{
    const status = await fetch(api("/api/kicad/revision")).then(r => r.json());
    if (!sourceSnapshot || status.changed || status.revision !== sourceSnapshot.revision) {{
      sourceSnapshot = undefined;
      await loadSources();
      viewerFrames().forEach(frame => void postSnapshot(frame));
      if (activeTab === "checks") await loadChecks();
    }}
  }} finally {{ refreshInFlight = false; }}
}};
const applyRevision = async event => {{
  if (!event?.revision) return;
  const sourceChanged = sourceSnapshot?.revision !== event.revision;
  if (sourceChanged) {{
    sourceSnapshot = undefined;
    await loadSources();
    // The retained native viewer prepares replacement sources internally and
    // keeps the previous canvas alive until the new parse/render is usable.
    viewerFrames().forEach(frame => void postSnapshot(frame));
  }}
  if (activeTab === "checks") await loadChecks();
  if (activeTab === "bom") await loadBom();
  if (activeTab === "libraries") await loadLibraries();
}};
let buildStageKeys = {{}};
let buildPublishKey;
const applyBuild = async event => {{
  window.dispatchEvent(new CustomEvent("kicad-pcb-build", {{detail:event}}));
  const done = {{}};
  for (const stage of event.status?.stages || []) {{
    if (stage.state === "ok" || stage.state === "failed") done[stage.stage] = stage.input_key;
  }}
  const changed = id => done[id] && buildStageKeys[id] !== done[id];
  const checks = changed("drc") || changed("erc") || changed("quality");
  const gerbers = changed("gerbers") || changed("jlcpcb");
  const model = changed("glb");
  buildStageKeys = {{...buildStageKeys, ...done}};
  const publishKey = JSON.stringify(event.status?.publish || {{}});
  const published = publishKey !== buildPublishKey;
  buildPublishKey = publishKey;
  if (checks && activeTab === "checks") await loadChecks();
  if (gerbers || published) await refreshGerbers();
  if (model) document.querySelectorAll("iframe.model-viewer").forEach(frame => void postSnapshot(frame));
}};
const connectEvents = () => {{
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const socket = new WebSocket(`${{proto}}//${{location.host}}/ws/events?${{projectParam()}}`);
  socket.onmessage = event => {{
    try {{
      const payload = JSON.parse(event.data);
      if (payload.type === "Revision") void applyRevision(payload);
      if (payload.type === "Build") void applyBuild(payload);
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
  if (id === "bom") void loadBom();
  if (id === "libraries") void loadLibraries();
  if (id === "gerbers") void loadGerbers();
}};
document.querySelectorAll(".tabs button").forEach(b => b.onclick = () => setTab(b.dataset.tab));
document.getElementById("projectSelect")?.addEventListener("change", event => {{
  const next = event.target.value;
  const url = new URL(location.href);
  url.searchParams.set("project", next);
  location.href = url.toString();
}});
setTab(document.getElementById(location.hash.slice(1)) ? location.hash.slice(1) : "schematic");
loadSources().then(() => viewerFrames().forEach(frame => void postSnapshot(frame))).catch(err => console.warn("KiCad PCB preload failed", err));
connectEvents();
document.addEventListener("visibilitychange", () => {{ if (!document.hidden) void refreshPane(); }});
</script><script src="/kicad-pcb/build-strip.js"></script></body></html>"##,
        session = escape(session),
        cwd = escape(&cwd.display().to_string()),
        tabs = tabs,
        files = files,
        project_id = project_id,
        project_attr = escape_attr(&project.id),
        project_options = project_options,
    )
}

fn selected_gerber_files(files: &[FileEntry]) -> Vec<String> {
    let generated_sets = [
        "fab/gerbers/",
        "fab/jlcpcb/",
        "fabrication/gerbers/",
        "fabrication/jlcpcb/",
    ];
    for prefix in generated_sets {
        let selected = gerber_files_with_prefix(files, prefix);
        if !selected.is_empty() {
            return selected;
        }
    }
    let mut selected = files
        .iter()
        .filter(|file| file.kind == "gerber" && is_embeddable_gerber_file(&file.path))
        .filter(|file| {
            !path_has_component(&file.path, &["tmp", "temp", "build", "backup", "backups"])
        })
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    selected.sort();
    selected
}

fn gerber_files_with_prefix(files: &[FileEntry], prefix: &str) -> Vec<String> {
    let mut selected = files
        .iter()
        .filter(|file| file.kind == "gerber")
        .filter(|file| file.path.starts_with(prefix))
        .filter(|file| is_embeddable_gerber_file(&file.path))
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    selected.sort();
    selected
}

fn is_embeddable_gerber_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(is_gerber_extension)
}

fn is_gerber_extension(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    matches!(
        ext.as_str(),
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
    ) || numbered_inner_gerber_extension(&ext)
}

fn numbered_inner_gerber_extension(ext: &str) -> bool {
    let Some(number) = ext.strip_prefix('g') else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn path_has_component(path: &str, components: &[&str]) -> bool {
    Path::new(path)
        .components()
        .any(|part| components.contains(&part.as_os_str().to_string_lossy().as_ref()))
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

fn escape_attr(value: &str) -> String {
    escape(value).replace('\'', "&#39;")
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_kicad10_reference_property_as_fp_text() {
        let source = r#"(footprint "LED_SMD:LED_1206_3216Metric"
	(layer "F.Cu")
	(property "Reference" "D2"
		(at 0 -2.1 0)
		(layer "F.SilkS")
		(uuid "6f2b23a9-7457-476d-8c69-8a57b2f3916a")
		(effects
			(font
				(size 0.85 0.85)
				(thickness 0.13)
			)
		)
	)
)
"#;

        let normalized = synthesize_kicad10_reference_text(source);

        assert!(normalized.contains("(property \"Reference\" \"D2\""));
        assert!(normalized.contains("(fp_text reference \"D2\""));
        assert!(normalized.contains("(layer \"F.SilkS\")"));
        assert!(normalized.contains("(size 0.85 0.85)"));
    }

    #[test]
    fn hidden_reference_property_is_not_synthesized() {
        let source = r#"(footprint "X"
	(property "Reference" "REF**"
		(at 0 0 0)
		(layer "F.SilkS")
		(hide yes)
	)
)
"#;

        let normalized = synthesize_kicad10_reference_text(source);

        assert!(normalized.contains("(property \"Reference\" \"REF**\""));
        assert!(!normalized.contains("(fp_text reference"));
    }

    #[test]
    fn numbered_kicad_inner_copper_gerbers_are_embeddable() {
        assert_eq!(kind_for(Path::new("module-In1_Cu.g1")), Some("gerber"));
        assert_eq!(kind_for(Path::new("module-In2_Cu.g2")), Some("gerber"));
        assert!(is_gerber_artifact(Path::new("module-In1_Cu.g1")));
        assert!(is_embeddable_gerber_file("fab/gerbers/module-In2_Cu.g2"));
        assert!(!is_embeddable_gerber_file("fab/gerbers/module-job.gbrjob"));
    }

    #[test]
    fn parses_symbol_footprint_and_model_links() {
        let tmp = TempDir::new().expect("temp dir");
        let schematic = tmp.path().join("board.kicad_sch");
        let pcb = tmp.path().join("board.kicad_pcb");
        std::fs::write(
            &schematic,
            r#"(kicad_sch
  (symbol (lib_id "Device:R")
    (property "Reference" "R1")
    (property "Value" "10k")
    (property "Footprint" "Resistor_SMD:R_0603_1608Metric")
  )
)
"#,
        )
        .expect("write schematic");
        std::fs::write(
            &pcb,
            r#"(kicad_pcb
  (footprint "Resistor_SMD:R_0603_1608Metric"
    (property "Reference" "R1")
    (property "Value" "10k")
    (model "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step")
  )
)
"#,
        )
        .expect("write pcb");

        let symbols = parse_schematic_symbols(&schematic).expect("symbols");
        let footprints = parse_pcb_footprints(&pcb).expect("footprints");

        assert_eq!(symbols[0].reference, "R1");
        assert_eq!(
            symbols[0].footprint.as_deref(),
            Some("Resistor_SMD:R_0603_1608Metric")
        );
        assert_eq!(footprints[0].reference, "R1");
        assert_eq!(footprints[0].footprint, "Resistor_SMD:R_0603_1608Metric");
        assert_eq!(
            footprints[0].models,
            vec!["${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step"]
        );
    }
}
