//! API types for the smart library view (`/api/kicad/library*`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryResponse {
    pub ok: bool,
    pub project: String,
    pub kicad_version: Option<String>,
    /// One entry per unique (symbol, footprint, model set) placed in the design.
    pub parts: Vec<LibraryPart>,
    /// Items in project-local/configured libraries that nothing references.
    pub unused: Vec<LibraryPart>,
    pub libraries: Vec<LibrarySource>,
    pub warnings: Vec<String>,
    pub stats: LibraryStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LibraryPart {
    pub id: String,
    pub title: String,
    pub symbol: Option<String>,
    pub footprint: Option<String>,
    pub refs: Vec<String>,
    pub values: Vec<String>,
    pub lcsc: Option<String>,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub datasheet: Option<String>,
    pub description: Option<String>,
    pub models: Vec<ModelRef>,
    pub badges: Vec<LibraryBadge>,
    pub thumbs: PartThumbs,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRef {
    pub path: String,
    pub resolved: Option<String>,
    pub found: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryBadge {
    /// Stable machine id, e.g. `footprint-drift`, `model-missing`.
    pub kind: String,
    /// `error`, `warning` or `info`.
    pub level: String,
    pub label: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PartThumbs {
    pub symbol: ThumbRef,
    pub footprint: ThumbRef,
    pub model: ThumbRef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThumbRef {
    /// Content-addressed cache key; absent when nothing can be rendered.
    pub key: Option<String>,
    pub url: Option<String>,
    /// `ready`, `queued`, `rendering`, `failed`, or `none` (placeholder).
    pub state: String,
    pub message: Option<String>,
    /// Interactive viewer source (KiCanvas `.kicad_sch`/`.kicad_pcb` or `.glb`).
    pub viewer: Option<String>,
    /// Which copy was rendered: `board`, `schematic`, or `library`.
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibrarySource {
    pub nickname: String,
    pub kind: String,
    pub path: String,
    pub local: bool,
    pub exists: bool,
    pub items: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LibraryStats {
    pub parts: usize,
    pub unused: usize,
    pub thumbs: usize,
    pub ready: usize,
    pub pending: usize,
    pub failed: usize,
    pub inventory_ms: u64,
    pub rendered: usize,
    pub cache_hits: usize,
    pub total_render_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThumbStatus {
    pub state: String,
    pub url: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LibraryStatusResponse {
    pub ok: bool,
    pub states: BTreeMap<String, ThumbStatus>,
    pub pending: usize,
    pub stats: LibraryStats,
}
