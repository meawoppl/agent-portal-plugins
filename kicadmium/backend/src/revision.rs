//! Content-hash source revisions and per-project source scoping.
//!
//! A project's revision is derived only from the relative path and contents of
//! the KiCad sources that feed its build stages, never from mtimes, so touching
//! a file without changing it does not produce a new revision. Sources are split
//! into a schematic set and a PCB set so build stages can depend on only what
//! they read.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::Result;
use sha2::{Digest, Sha256};
use shared::SourceHashes;
use walkdir::WalkDir;

use crate::ProjectContext;

/// Which source set(s) a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Sch,
    Pcb,
    Both,
}

impl SourceKind {
    fn sch(self) -> bool {
        matches!(self, Self::Sch | Self::Both)
    }
    fn pcb(self) -> bool {
        matches!(self, Self::Pcb | Self::Both)
    }
}

/// How a changed path relates to a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventScope {
    None,
    Source,
    Artifact,
}

/// Resolved source files for one project.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectSources {
    /// Hash key (project-relative path, or `@repo/<path>` outside the root) to
    /// absolute path.
    pub sch: BTreeMap<String, PathBuf>,
    pub pcb: BTreeMap<String, PathBuf>,
    /// Library directories outside the project scope (from lib tables or
    /// `.kicad-pcb.json` libraries) whose contents feed the project.
    pub external_dirs: Vec<PathBuf>,
}

impl ProjectSources {
    fn contains(&self, path: &Path) -> bool {
        self.sch.values().any(|item| item == path) || self.pcb.values().any(|item| item == path)
    }
}

/// Full hashing result for one project.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectHashes {
    pub hashes: SourceHashes,
    /// Hash key to file sha256 for every source in either set.
    pub files: BTreeMap<String, String>,
}

/// Returns content hashes for the project and refreshes the cached source
/// scope used by the file watcher.
pub(crate) fn project_hashes(project: &ProjectContext) -> Result<ProjectHashes> {
    let sources = collect_sources(project);
    let mut files = BTreeMap::new();
    let sch = set_hash(&sources.sch, &mut files);
    let pcb = set_hash(&sources.pcb, &mut files);
    let revision = combined_revision(&sch, &pcb);
    scope_cache()
        .lock()
        .expect("scope cache poisoned")
        .insert(project.id.clone(), Arc::new(sources));
    Ok(ProjectHashes {
        hashes: SourceHashes { revision, sch, pcb },
        files,
    })
}

pub(crate) fn combined_revision(sch: &str, pcb: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"kicad-pcb-revision/v2\0sch\0");
    digest.update(sch.as_bytes());
    digest.update(b"\0pcb\0");
    digest.update(pcb.as_bytes());
    hex(&digest.finalize())[..20].to_string()
}

fn set_hash(set: &BTreeMap<String, PathBuf>, files: &mut BTreeMap<String, String>) -> String {
    let mut digest = Sha256::new();
    for (key, path) in set {
        let Some(sha) = file_sha256(path) else {
            continue;
        };
        digest.update(key.as_bytes());
        digest.update([0]);
        digest.update(sha.as_bytes());
        digest.update([0]);
        files.insert(key.clone(), sha);
    }
    hex(&digest.finalize())
}

/// Sha256 of a file, memoized by (len, mtime) so unchanged files are not
/// re-read. A touched file is re-read but hashes to the same value.
pub(crate) fn file_sha256(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let stamp = (
        meta.len(),
        meta.modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or_default(),
    );
    type Stamp = (u64, u128);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Stamp, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((cached_stamp, sha)) = cache.lock().ok()?.get(path) {
        if *cached_stamp == stamp {
            return Some(sha.clone());
        }
    }
    let bytes = std::fs::read(path).ok()?;
    let sha = hex(&Sha256::digest(&bytes));
    cache
        .lock()
        .ok()?
        .insert(path.to_path_buf(), (stamp, sha.clone()));
    Some(sha)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn scope_cache() -> &'static Mutex<HashMap<String, Arc<ProjectSources>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<ProjectSources>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached_sources(project: &ProjectContext) -> Arc<ProjectSources> {
    if let Some(found) = scope_cache()
        .lock()
        .expect("scope cache poisoned")
        .get(&project.id)
    {
        return found.clone();
    }
    let sources = Arc::new(collect_sources(project));
    scope_cache()
        .lock()
        .expect("scope cache poisoned")
        .insert(project.id.clone(), sources.clone());
    sources
}

/// Source set membership by filename.
pub(crate) fn source_kind(path: &Path) -> Option<SourceKind> {
    let name = path.file_name()?.to_str()?;
    match name {
        "sym-lib-table" => return Some(SourceKind::Sch),
        "fp-lib-table" => return Some(SourceKind::Pcb),
        _ => {}
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "kicad_sch" | "kicad_sym" => SourceKind::Sch,
        "kicad_pcb" | "kicad_mod" | "kicad_dru" => SourceKind::Pcb,
        "kicad_pro" | "kicad_wks" => SourceKind::Both,
        _ => return None,
    })
}

/// True when `path` lies inside the project's own scope: under its root, not
/// inside a nested project root, and not in ignored/backup locations.
pub(crate) fn in_project_scope(project: &ProjectContext, path: &Path) -> bool {
    path.starts_with(&project.root)
        && !project
            .excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
        && crate::is_project_file(&project.root, path, true)
}

/// Classifies a changed path relative to a project for watch scoping.
pub(crate) fn event_scope(project: &ProjectContext, path: &Path) -> EventScope {
    let sources = cached_sources(project);
    if sources.contains(path)
        || (in_project_scope(project, path) && source_kind(path).is_some())
        || (source_kind(path).is_some()
            && sources
                .external_dirs
                .iter()
                .any(|dir| path.starts_with(dir)))
    {
        return EventScope::Source;
    }
    if path.starts_with(&project.root)
        && !project
            .excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
        && crate::is_project_file(&project.root, path, false)
    {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(
            crate::kind_for(path),
            Some("bom" | "placement" | "csv" | "gerber" | "model" | "netlist")
        ) || name == crate::artifacts::MANIFEST_NAME
            || name == "revision-sha256.json"
        {
            return EventScope::Artifact;
        }
    }
    EventScope::None
}

/// Walks the project scope and resolves library/model references.
pub(crate) fn collect_sources(project: &ProjectContext) -> ProjectSources {
    let mut sources = ProjectSources::default();
    for path in walk_scope(project) {
        if let Some(kind) = source_kind(&path) {
            add(&mut sources, project, kind, path);
        }
    }

    // Library tables can point outside the project root (for example a shared
    // repo-level library via ${KIPRJMOD}/../../libraries).
    for (table, kind) in [
        ("sym-lib-table", SourceKind::Sch),
        ("fp-lib-table", SourceKind::Pcb),
    ] {
        let path = project.root.join(table);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for uri in lib_table_uris(&text) {
            if let Some(resolved) = resolve_reference(project, &uri) {
                add_library(&mut sources, project, kind, resolved);
            }
        }
    }

    // Libraries declared in .kicad-pcb.json.
    let mut declared = Vec::new();
    if let Some(libraries) = &project.config.libraries {
        declared.push((project.root.clone(), libraries.clone()));
    }
    if project.config.inherit_libraries.unwrap_or(true) {
        declared.push((project.repo_root.clone(), project.shared_libraries.clone()));
    }
    for (base, libraries) in declared {
        for (paths, kind) in [
            (libraries.symbols.unwrap_or_default(), SourceKind::Sch),
            (libraries.footprints.unwrap_or_default(), SourceKind::Pcb),
        ] {
            for path in paths {
                if let Some(resolved) = lexical_join(&base, &path) {
                    add_library(&mut sources, project, kind, resolved);
                }
            }
        }
    }

    // Project-local 3D models referenced by boards (system model libraries are
    // part of the KiCad install and are covered by the KiCad version key).
    let boards: Vec<PathBuf> = sources
        .pcb
        .values()
        .filter(|path| path.extension().and_then(|v| v.to_str()) == Some("kicad_pcb"))
        .cloned()
        .collect();
    for board in boards {
        let Ok(text) = std::fs::read_to_string(&board) else {
            continue;
        };
        for model in model_references(&text) {
            let Some(resolved) = resolve_reference(project, &model) else {
                continue;
            };
            for candidate in model_candidates(&resolved) {
                if candidate.is_file() {
                    add(&mut sources, project, SourceKind::Pcb, candidate);
                }
            }
        }
    }
    sources.external_dirs.sort();
    sources.external_dirs.dedup();
    sources
}

/// Files in the project's own scope, pruning ignored dirs and nested projects.
pub(crate) fn walk_scope(project: &ProjectContext) -> Vec<PathBuf> {
    let root = project.root.clone();
    let excluded = project.excluded_roots.clone();
    let mut files = WalkDir::new(&root)
        .into_iter()
        .filter_entry(|entry| {
            let path = entry.path();
            if path == root {
                return true;
            }
            if excluded.iter().any(|item| path.starts_with(item)) {
                return false;
            }
            !entry.file_type().is_dir() || crate::is_project_file(&root, path, true)
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| crate::is_project_file(&root, path, true))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn add(sources: &mut ProjectSources, project: &ProjectContext, kind: SourceKind, path: PathBuf) {
    let key = hash_key(project, &path);
    if kind.sch() {
        sources.sch.insert(key.clone(), path.clone());
    }
    if kind.pcb() {
        sources.pcb.insert(key, path);
    }
}

fn add_library(
    sources: &mut ProjectSources,
    project: &ProjectContext,
    kind: SourceKind,
    path: PathBuf,
) {
    if !path.starts_with(&project.repo_root) {
        return;
    }
    if path.is_file() {
        if !in_project_scope(project, &path) {
            add(sources, project, kind, path);
        }
        return;
    }
    if !path.is_dir() || in_project_scope(project, &path.join("x")) {
        // Directories inside the project scope are already walked.
        return;
    }
    sources.external_dirs.push(path.clone());
    for entry in WalkDir::new(&path).into_iter().filter_map(Result::ok) {
        let file = entry.path();
        if entry.file_type().is_file() && source_kind(file).is_some() {
            add(sources, project, kind, file.to_path_buf());
        }
    }
}

fn hash_key(project: &ProjectContext, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(&project.root) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    if let Ok(rel) = path.strip_prefix(&project.repo_root) {
        return format!("@repo/{}", rel.to_string_lossy().replace('\\', "/"));
    }
    path.to_string_lossy().into_owned()
}

fn lib_table_uris(text: &str) -> Vec<String> {
    let mut uris = Vec::new();
    let mut offset = 0;
    while let Some(found) = text[offset..].find("(uri ") {
        let start = offset + found + "(uri ".len();
        if let Some(value) = crate::quoted_fields(&text[start..]).into_iter().next() {
            uris.push(value);
        }
        offset = start;
    }
    uris
}

fn model_references(text: &str) -> Vec<String> {
    let mut models = Vec::new();
    let mut offset = 0;
    while let Some(found) = text[offset..].find("(model ") {
        let start = offset + found + "(model ".len();
        let rest = &text[start..];
        let line = rest.lines().next().unwrap_or_default();
        if let Some(value) = crate::quoted_fields(line).into_iter().next() {
            models.push(value);
        }
        offset = start;
    }
    models.sort();
    models.dedup();
    models
}

fn model_candidates(path: &Path) -> Vec<PathBuf> {
    // --subst-models swaps VRML for STEP when a sibling exists.
    let mut candidates = vec![path.to_path_buf()];
    for ext in ["step", "stp", "wrl"] {
        let alt = path.with_extension(ext);
        if alt != path {
            candidates.push(alt);
        }
    }
    candidates
}

/// Resolves a KiCad path reference (`${KIPRJMOD}/...` or relative) to an
/// absolute path inside the repo. Other environment variables refer to the
/// KiCad install and are not tracked.
fn resolve_reference(project: &ProjectContext, value: &str) -> Option<PathBuf> {
    let rest = if let Some(rest) = value
        .strip_prefix("${KIPRJMOD}")
        .or_else(|| value.strip_prefix("$(KIPRJMOD)"))
    {
        rest.trim_start_matches(['/', '\\'])
    } else if value.contains("${") || value.contains("$(") {
        return None;
    } else {
        value
    };
    let resolved = if Path::new(rest).is_absolute() {
        normalize(Path::new(rest))
    } else {
        lexical_join(&project.root, Path::new(rest))?
    };
    resolved.starts_with(&project.repo_root).then_some(resolved)
}

fn lexical_join(base: &Path, rel: &Path) -> Option<PathBuf> {
    Some(normalize(&base.join(rel)))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use crate::ProjectConfig;

    pub(crate) fn project(repo: &Path, id: &str, root: &str) -> ProjectContext {
        project_excluding(repo, id, root, &[])
    }

    pub(crate) fn project_excluding(
        repo: &Path,
        id: &str,
        root: &str,
        excluded: &[&str],
    ) -> ProjectContext {
        ProjectContext {
            id: id.to_string(),
            name: id.to_string(),
            root: normalize(&repo.join(root)),
            config: ProjectConfig {
                id: id.to_string(),
                root: PathBuf::from(root),
                ..ProjectConfig::default()
            },
            shared_libraries: Default::default(),
            repo_root: repo.to_path_buf(),
            excluded_roots: excluded.iter().map(|item| repo.join(item)).collect(),
            quality: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{project, project_excluding};
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn revision_ignores_touch_and_changes_on_edit() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        write(&repo.join("b.kicad_sch"), "(kicad_sch)");
        write(&repo.join("b.kicad_pcb"), "(kicad_pcb)");
        let project = project(&repo, "b", ".");
        let first = project_hashes(&project).unwrap().hashes;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let file = std::fs::File::options()
            .write(true)
            .open(repo.join("b.kicad_pcb"))
            .unwrap();
        file.set_modified(std::time::SystemTime::now()).unwrap();
        drop(file);
        let touched = project_hashes(&project).unwrap().hashes;
        assert_eq!(first, touched, "touch must not change revision");

        write(&repo.join("b.kicad_pcb"), "(kicad_pcb (edited))");
        let edited = project_hashes(&project).unwrap().hashes;
        assert_ne!(first.revision, edited.revision);
        assert_ne!(first.pcb, edited.pcb);
        assert_eq!(first.sch, edited.sch, "PCB edit must not change sch hash");
    }

    #[test]
    fn pro_file_feeds_both_sets() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        write(&repo.join("b.kicad_pro"), "{}");
        let project = project(&repo, "b", ".");
        let first = project_hashes(&project).unwrap().hashes;
        write(&repo.join("b.kicad_pro"), "{\"x\":1}");
        let second = project_hashes(&project).unwrap().hashes;
        assert_ne!(first.sch, second.sch);
        assert_ne!(first.pcb, second.pcb);
    }

    #[test]
    fn root_project_excludes_nested_project_roots() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        write(&repo.join("top.kicad_pcb"), "(kicad_pcb)");
        write(&repo.join("boards/a/a.kicad_pcb"), "(kicad_pcb)");
        write(&repo.join("tmp/scratch.kicad_pcb"), "(kicad_pcb)");
        write(&repo.join(".worktrees/x/top.kicad_pcb"), "(kicad_pcb)");
        let root = project_excluding(&repo, "root", ".", &["boards/a"]);
        let nested = project(&repo, "a", "boards/a");

        let sources = collect_sources(&root);
        assert_eq!(
            sources.pcb.keys().collect::<Vec<_>>(),
            vec!["top.kicad_pcb"]
        );

        let nested_file = repo.join("boards/a/a.kicad_pcb");
        assert_eq!(event_scope(&root, &nested_file), EventScope::None);
        assert_eq!(event_scope(&nested, &nested_file), EventScope::Source);
        assert_eq!(
            event_scope(&root, &repo.join("top.kicad_pcb")),
            EventScope::Source
        );
        assert_eq!(
            event_scope(&nested, &repo.join("top.kicad_pcb")),
            EventScope::None
        );
        assert_eq!(
            event_scope(&root, &repo.join("fab/gerbers/top-F_Cu.gtl")),
            EventScope::Artifact
        );
        assert_eq!(
            event_scope(&root, &repo.join("boards/a/fab/gerbers/a-F_Cu.gtl")),
            EventScope::None
        );
    }

    #[test]
    fn external_libraries_and_models_are_tracked() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap();
        write(
            &repo.join("boards/a/fp-lib-table"),
            "(fp_lib_table (lib (name \"S\")(type \"KiCad\")(uri \"${KIPRJMOD}/../../libs/S.pretty\")(options \"\")(descr \"\")) (lib (name \"K\")(type \"KiCad\")(uri \"${KICAD9_FOOTPRINT_DIR}/K.pretty\")))",
        );
        write(&repo.join("libs/S.pretty/R.kicad_mod"), "(footprint \"R\")");
        write(
            &repo.join("boards/a/a.kicad_pcb"),
            "(kicad_pcb (footprint \"R\" (model \"${KIPRJMOD}/3d/R.step\" (offset))))",
        );
        write(&repo.join("boards/a/3d/R.step"), "STEP");
        let project = project(&repo, "a", "boards/a");
        let hashes = project_hashes(&project).unwrap();
        assert!(hashes.files.contains_key("@repo/libs/S.pretty/R.kicad_mod"));
        assert!(hashes.files.contains_key("3d/R.step"));
        assert_eq!(
            event_scope(&project, &repo.join("libs/S.pretty/New.kicad_mod")),
            EventScope::Source
        );
        let before = hashes.hashes;
        write(&repo.join("boards/a/3d/R.step"), "STEP2");
        let after = project_hashes(&project).unwrap().hashes;
        assert_ne!(before.pcb, after.pcb);
        assert_eq!(before.sch, after.sch);
    }
}
