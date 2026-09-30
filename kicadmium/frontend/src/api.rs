//! Typed fetch helpers for the backend's `/api/*` routes. Every project-scoped
//! call takes the active project id ("" selects the server default).
#![allow(dead_code)] // TODO(kc-claude): drop once every tab consumes its helpers.

use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;
use shared::library::{LibraryResponse, LibraryStatusResponse};
use shared::{
    BuildStatus, CheckResponse, GerberSourcesResponse, HealthResponse, ManifestResponse,
    RevisionResponse, SourcesResponse, WorkspaceResponse,
};

/// Percent-encode a query component (RFC 3986 unreserved characters pass).
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Append `project=<id>` to `path` (no-op for the default project).
pub fn url(path: &str, project: &str) -> String {
    if project.is_empty() {
        return path.to_string();
    }
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}project={}", encode(project))
}

async fn checked(response: Result<Response, gloo_net::Error>) -> Result<Response, String> {
    let response = response.map_err(|err| err.to_string())?;
    if response.ok() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    // Backend errors are `{"ok":false,"message":...}`.
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or(body);
    Err(format!("HTTP {status}: {message}"))
}

pub async fn get_json<T: DeserializeOwned>(path: &str, project: &str) -> Result<T, String> {
    checked(Request::get(&url(path, project)).send().await)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

pub async fn get_text(path: &str, project: &str) -> Result<String, String> {
    checked(Request::get(&url(path, project)).send().await)
        .await?
        .text()
        .await
        .map_err(|err| err.to_string())
}

pub async fn get_bytes(full_url: &str) -> Result<Vec<u8>, String> {
    checked(Request::get(full_url).send().await)
        .await?
        .binary()
        .await
        .map_err(|err| err.to_string())
}

pub async fn post_json<T: DeserializeOwned>(path: &str, project: &str) -> Result<T, String> {
    checked(Request::post(&url(path, project)).send().await)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

pub async fn health() -> Result<HealthResponse, String> {
    get_json("/healthz", "").await
}

pub async fn manifest(project: &str) -> Result<ManifestResponse, String> {
    get_json("/api/kicad/manifest", project).await
}

pub async fn workspace() -> Result<WorkspaceResponse, String> {
    get_json("/api/projects", "").await
}

pub async fn revision(project: &str) -> Result<RevisionResponse, String> {
    get_json("/api/kicad/revision", project).await
}

pub async fn sources(project: &str) -> Result<SourcesResponse, String> {
    get_json("/api/kicad/sources", project).await
}

pub fn model_glb_url(project: &str, revision: &str) -> String {
    url(
        &format!("/api/kicad/model.glb?rev={}", encode(revision)),
        project,
    )
}

/// `kind` is `drc`, `erc`, or `quality`.
pub async fn check(kind: &str, project: &str) -> Result<CheckResponse, String> {
    get_json(&format!("/api/kicad/{kind}"), project).await
}

pub async fn lint(project: &str) -> Result<serde_json::Value, String> {
    get_json("/api/kicad/lint", project).await
}

pub fn lint_contact_sheet_url(project: &str) -> String {
    url("/api/kicad/lint/contact-sheet", project)
}

/// Server-rendered BOM/CPL tables (HTML fragment).
pub async fn bom_html(project: &str) -> Result<String, String> {
    get_text("/api/kicad/bom", project).await
}

pub async fn library(project: &str) -> Result<LibraryResponse, String> {
    get_json("/api/kicad/library", project).await
}

pub async fn library_status(project: &str) -> Result<LibraryStatusResponse, String> {
    get_json("/api/kicad/library/status", project).await
}

pub async fn gerbers(project: &str) -> Result<GerberSourcesResponse, String> {
    get_json("/api/kicad/gerbers", project).await
}

pub async fn build_status(project: &str) -> Result<BuildStatus, String> {
    get_json("/api/build/status", project).await
}

pub async fn build_run(project: &str, force: bool) -> Result<BuildStatus, String> {
    let path = if force {
        "/api/build/run?force=1"
    } else {
        "/api/build/run"
    };
    post_json(path, project).await
}

/// Copies the current build into the configured fab dirs; returns the
/// server's publish report (`files` = copied count).
pub async fn build_publish(project: &str) -> Result<serde_json::Value, String> {
    post_json("/api/build/publish", project).await
}

pub fn build_artifact_url(project: &str, stage: &str, file: &str) -> String {
    url(
        &format!(
            "/api/build/artifact?stage={}&file={}&download=1",
            encode(stage),
            encode(file)
        ),
        project,
    )
}

pub fn events_ws_url(project: &str) -> String {
    let location = web_sys::window().expect("window").location();
    let scheme = if location.protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let host = location.host().unwrap_or_default();
    format!("{scheme}://{host}{}", url("/ws/events", project))
}
