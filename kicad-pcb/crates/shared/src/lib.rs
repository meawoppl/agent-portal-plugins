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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_roundtrip() {
        let value = HealthResponse {
            ok: true,
            plugin: "kicad-pcb".to_string(),
            kicad_cli: true,
            preloaded: true,
            source_revision: Some("1-abcd".to_string()),
            warmed_at_ms: Some(42),
        };
        let json = serde_json::to_string(&value).unwrap();
        let parsed: HealthResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
    }
}
