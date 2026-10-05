//! Project-aware wrapper around the `kct lint` heuristic engine (`kct::lint`).
//!
//! Resolves the project's canonical `.kicad_pcb`, uses the project id as the
//! stable board identity, and reads optional `lint.config` / `lint.reviews`
//! paths (project-root relative) from `.kicad-pcb.json`. Findings are advisory
//! heuristics, never native DRC, and never modify the board.

use std::{ffi::OsString, path::PathBuf};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::header,
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use kct::lint::{
    board_file::{self, BoardLintFile, Checked},
    review, Config, Report,
};
use serde::{Deserialize, Serialize};

use crate::{
    pick_project_file, selected_project, AppError, AppState, ProjectContext, ProjectQuery,
};

/// `lint` section of a project in `.kicad-pcb.json`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct LintConfig {
    /// kct lint JSON `Config`; omitted fields use kct lint defaults.
    pub config: Option<PathBuf>,
    /// Evidence-bound review ledger; stale evidence is not reused.
    pub reviews: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum FailOn {
    #[default]
    Never,
    Warning,
    Error,
}

pub(crate) struct LintRun {
    pub source: String,
    pub file: BoardLintFile,
    pub checked: Checked,
}

pub(crate) fn run(project: &ProjectContext) -> Result<LintRun> {
    let board = pick_project_file(project, "kicad_pcb")?
        .ok_or_else(|| anyhow!("project {} has no .kicad_pcb to lint", project.id))?;
    let source =
        std::fs::read_to_string(&board).with_context(|| format!("reading {}", board.display()))?;
    let policy_path = BoardLintFile::path_for(&board);
    let settings = project.config.lint.clone().unwrap_or_default();
    let legacy_config = match &settings.config {
        Some(path) => {
            let path = project.root.join(path);
            serde_json::from_str::<Config>(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
            )
            .with_context(|| format!("parsing {}", path.display()))?
        }
        None => Config::default(),
    };
    let legacy_ledger = if let Some(path) = &settings.reviews {
        let path = project.root.join(path);
        if path.exists() {
            Some(review::load(&path)?)
        } else {
            None
        }
    } else {
        None
    };
    let mut file = if policy_path.exists() {
        BoardLintFile::load_or_default(&policy_path, &project.id)?
    } else {
        BoardLintFile::migrate(Some(legacy_config), legacy_ledger, &project.id)
    };
    let checked = board_file::lint_board(&source, &file, review::now())?;
    if policy_path.exists() && board_file::backfill(&mut file, &checked.report) > 0 {
        file.save(&policy_path)?;
    }
    Ok(LintRun {
        source,
        file,
        checked,
    })
}

impl FailOn {
    fn as_arg(self) -> &'static str {
        match self {
            FailOn::Never => "never",
            FailOn::Warning => "warning",
            FailOn::Error => "error",
        }
    }
}

/// `kicadmium lint`: resolve the project's board and run `kct lint run` on it,
/// so the CLI honours `<board>.lint.json` and prints exactly what kct does.
pub(crate) fn run_cli(
    project: &ProjectContext,
    json: bool,
    contact_sheet: Option<PathBuf>,
    fail_on: FailOn,
) -> Result<i32> {
    let board = pick_project_file(project, "kicad_pcb")?
        .ok_or_else(|| anyhow!("project {} has no .kicad_pcb to lint", project.id))?;
    let settings = project.config.lint.clone().unwrap_or_default();
    if !BoardLintFile::path_for(&board).exists()
        && (settings.config.is_some() || settings.reviews.is_some())
    {
        eprintln!(
            "warning: legacy .kicad-pcb.json lint.config/lint.reviews are not applied here \
             (only the workbench reads them). Create {} with `kicadmium kct -- lint init` \
             and move the settings into it.",
            BoardLintFile::path_for(&board).display()
        );
    }
    let mut args: Vec<OsString> = vec![
        "lint".into(),
        "run".into(),
        board.into_os_string(),
        "--format".into(),
        if json { "json" } else { "text" }.into(),
        "--fail-on".into(),
        fail_on.as_arg().into(),
    ];
    if let Some(sheet) = contact_sheet {
        args.extend(["--contact-sheet".into(), sheet.into_os_string()]);
    }
    kct::cli::run(args)
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/kicad/lint", get(report_endpoint))
        .route("/api/kicad/lint/contact-sheet", get(contact_sheet_endpoint))
        .route("/api/kicad/lint/exceptions", get(exceptions_sheet_endpoint))
}

async fn lint_blocking(state: &AppState, query: &ProjectQuery) -> Result<LintRun, AppError> {
    let project = selected_project(state, query.project.as_deref())?;
    Ok(tokio::task::spawn_blocking(move || run(&project)).await??)
}

async fn report_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Report>, AppError> {
    Ok(Json(lint_blocking(&state, &query).await?.checked.report))
}

async fn contact_sheet_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Response, AppError> {
    let run = lint_blocking(&state, &query).await?;
    let html = kct::lint::contact_sheet::render(&run.source, &run.checked.report)?;
    Ok(([(header::CACHE_CONTROL, "no-cache")], Html(html)).into_response())
}

async fn exceptions_sheet_endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Response, AppError> {
    let run = lint_blocking(&state, &query).await?;
    let html = kct::lint::contact_sheet::render_exceptions(
        &run.source,
        &run.checked.report,
        &run.file,
        &run.checked.audit,
    )?;
    Ok(([(header::CACHE_CONTROL, "no-cache")], Html(html)).into_response())
}
