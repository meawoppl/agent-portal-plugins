pub mod library;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub kind: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub size: u64,
    #[serde(rename = "mtimeMs")]
    pub mtime_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFile {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestResponse {
    pub root: String,
    pub revision: String,
    pub files: Vec<FileEntry>,
    pub warnings: Vec<String>,
    pub kicad_cli: Option<String>,
    pub kicad_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub ok: bool,
    pub plugin: String,
    pub kicad_cli: bool,
    pub preloaded: bool,
    pub source_revision: Option<String>,
    pub warmed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevisionResponse {
    pub ok: bool,
    pub revision: String,
    pub changed: bool,
    pub warmed_revision: Option<String>,
    pub warmed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourcesResponse {
    pub ok: bool,
    pub revision: String,
    pub sources: Vec<SourceFile>,
    pub warmed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckResponse {
    pub ok: bool,
    pub source: Option<String>,
    pub command: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub report: String,
    pub message: Option<String>,
    pub tool: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ServerEvent {
    Revision {
        /// Project id the revision belongs to. Older clients ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        revision: String,
        previous_revision: Option<String>,
        warmed_at_ms: u64,
        reason: String,
    },
    /// Build pipeline status for one project changed.
    Build {
        project: String,
        status: BuildStatus,
    },
}

/// Content hashes of the KiCad sources that feed build stages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceHashes {
    /// Combined revision (schematic + PCB sets).
    pub revision: String,
    /// Schematic set: `.kicad_sch`, symbol libraries, `sym-lib-table`, `.kicad_pro`.
    pub sch: String,
    /// PCB set: `.kicad_pcb`, footprint libraries, `fp-lib-table`, `.kicad_dru`,
    /// `.kicad_pro`, referenced project-local 3D models.
    pub pcb: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageStatus {
    pub stage: String,
    pub label: String,
    /// `queued`, `running`, `ok`, `failed`, `stale`, `cancelled`, or `disabled`.
    pub state: String,
    pub input_key: String,
    pub cached: bool,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub message: Option<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishStatus {
    /// A plugin publish manifest exists for the project.
    pub published: bool,
    pub manifest: Option<String>,
    pub published_at_ms: Option<u64>,
    pub published_revision: Option<String>,
    /// Published fab outputs no longer match the current sources.
    pub stale: bool,
    /// Build stages whose published inputs differ from the current sources.
    pub stale_stages: Vec<String>,
    /// Human-readable staleness reasons, including legacy
    /// `revision-sha256.json` comparisons.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildStatus {
    pub project: String,
    pub hashes: SourceHashes,
    pub busy: bool,
    pub auto_publish: bool,
    pub stages: Vec<StageStatus>,
    pub publish: PublishStatus,
    pub updated_ms: u64,
}

/// One board/project in the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    /// Project root relative to the workspace root ("" for the root itself).
    pub root: String,
}

/// `/api/projects`: the workspace the server was started on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceResponse {
    pub cwd: String,
    /// Agent Portal session id, when launched as a Portal surface.
    pub session: String,
    pub default_project: String,
    pub projects: Vec<ProjectSummary>,
}

/// One Gerber/drill file the Gerber tab should load.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GerberSource {
    pub path: String,
    pub url: String,
}

/// `/api/kicad/gerbers`: the current build's Gerbers when ready, else the
/// published files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GerberSourcesResponse {
    /// `build`, `published`, or `none`.
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub revision: Option<String>,
    pub label: String,
    pub files: Vec<GerberSource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_roundtrip() {
        let value = HealthResponse {
            ok: true,
            plugin: "kicadmium".to_string(),
            kicad_cli: true,
            preloaded: true,
            source_revision: Some("1-abcd".to_string()),
            warmed_at_ms: Some(42),
        };
        let json = serde_json::to_string(&value).unwrap();
        let parsed: HealthResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn server_event_roundtrip() {
        let value = ServerEvent::Revision {
            project: Some("board".to_string()),
            revision: "2-abcd".to_string(),
            previous_revision: Some("1-abcd".to_string()),
            warmed_at_ms: 99,
            reason: "watch".to_string(),
        };
        let json = serde_json::to_string(&value).unwrap();
        let parsed: ServerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn revision_event_without_project_still_parses() {
        let json = r#"{"type":"Revision","revision":"r","previous_revision":null,"warmed_at_ms":1,"reason":"initial"}"#;
        let parsed: ServerEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(
            parsed,
            ServerEvent::Revision { project: None, .. }
        ));
    }
}
