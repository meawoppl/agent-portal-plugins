//! Per-board lint file (`<pcb_stem>.lint.json`, schema 2): configuration plus
//! evidence-bound exceptions, committed next to the board as the single source of
//! truth. Exceptions are audited against every run so stale entries surface.
use crate::lint::{
    copper::Extra, lint, model::Board, model::Point, review, rules, Config, Finding, Report,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize, Serializer};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

pub const SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Ignore,
    Flag,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Flag => "flag",
        }
    }
}
impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A reviewed non-issue (`ignore`) or acknowledged issue (`flag`), bound to the
/// finding key and evidence hash. Card metadata is the last-known finding state
/// at upsert time so stale exceptions still render meaningfully.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub key: String,
    /// Empty only for exceptions migrated from a schema-1 ledger and not yet backfilled.
    #[serde(default)]
    pub rule: String,
    #[serde(default)]
    pub subjects: Vec<String>,
    pub evidence: String,
    pub action: Action,
    pub reason: String,
    pub reviewer: String,
    pub reviewed_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub severity: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardLintFile {
    pub schema: u32,
    pub board_id: String,
    /// Partial on disk: omitted fields are defaults, and only non-default fields are written.
    #[serde(default, serialize_with = "partial_config")]
    pub config: Config,
    #[serde(default)]
    pub exceptions: Vec<Exception>,
}

fn partial_config<S: Serializer>(c: &Config, s: S) -> std::result::Result<S::Ok, S::Error> {
    use serde::ser::Error;
    let full = serde_json::to_value(c).map_err(S::Error::custom)?;
    let defaults = serde_json::to_value(Config::default()).map_err(S::Error::custom)?;
    let mut out = serde_json::Map::new();
    if let (serde_json::Value::Object(full), serde_json::Value::Object(defaults)) = (full, defaults)
    {
        for (k, v) in full {
            if defaults.get(&k) != Some(&v) {
                out.insert(k, v);
            }
        }
    }
    out.serialize(s)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExceptionStatus {
    /// Finding raised with identical evidence; its state is ignored/flagged.
    Applied,
    /// Finding raised but evidence changed; the finding stays open.
    Changed,
    /// Rule evaluated, finding not raised, every stored subject still on the board.
    Resolved,
    /// Rule evaluated, finding not raised, and stored subject geometry is gone.
    Orphaned,
    /// Decision past its expiry.
    Expired,
    /// Finding not raised but the rule was not fully evaluated (disabled, needs
    /// input, unsupported geometry, budget exhausted). Never treated as a fix.
    Unverified,
}
impl ExceptionStatus {
    pub const ALL: [Self; 6] = [
        Self::Applied,
        Self::Changed,
        Self::Resolved,
        Self::Orphaned,
        Self::Expired,
        Self::Unverified,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Changed => "changed",
            Self::Resolved => "resolved",
            Self::Orphaned => "orphaned",
            Self::Expired => "expired",
            Self::Unverified => "unverified",
        }
    }
    /// Orphaned, resolved and expired exceptions no longer key to a live finding.
    pub fn is_stale(self) -> bool {
        matches!(self, Self::Orphaned | Self::Resolved | Self::Expired)
    }
}
impl fmt::Display for ExceptionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExceptionAudit {
    pub key: String,
    pub rule: String,
    pub action: Action,
    pub reason: String,
    pub reviewer: String,
    pub status: ExceptionStatus,
    pub detail: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Checked {
    pub report: Report,
    pub audit: Vec<ExceptionAudit>,
}
impl Checked {
    /// Orphaned, resolved or expired exceptions; callers warn on these.
    pub fn stale(&self) -> impl Iterator<Item = &ExceptionAudit> {
        self.audit.iter().filter(|a| a.status.is_stale())
    }
}

impl BoardLintFile {
    pub fn new(board_id: &str) -> Self {
        Self {
            schema: SCHEMA,
            board_id: board_id.into(),
            config: Config::default(),
            exceptions: vec![],
        }
    }
    /// `boards/foo.kicad_pcb` -> `boards/foo.lint.json`.
    pub fn path_for(pcb: &Path) -> PathBuf {
        pcb.with_extension("lint.json")
    }
    pub fn load_or_default(path: &Path, default_board_id: &str) -> Result<Self> {
        if !path.exists() {
            let file = Self::new(default_board_id);
            file.validate()?;
            return Ok(file);
        }
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        match value.get("schema").and_then(|s| s.as_u64()) {
            Some(2) => {}
            Some(s) => bail!(
                "{}: unsupported lint file schema {s} (expected {SCHEMA}; migrate schema-1 config and reviews)",
                path.display()
            ),
            None => bail!("{}: missing lint file schema", path.display()),
        }
        let file: Self =
            serde_json::from_value(value).with_context(|| format!("parsing {}", path.display()))?;
        file.validate()
            .with_context(|| format!("validating {}", path.display()))?;
        Ok(file)
    }
    /// Build a schema-2 file from a legacy `--config` JSON and `--reviews` ledger.
    /// Decisions for other boards are dropped. Migrated exceptions have no rule or
    /// subjects until [`backfill`] sees their finding raised.
    pub fn migrate(
        config: Option<Config>,
        ledger: Option<review::Ledger>,
        board_id: &str,
    ) -> BoardLintFile {
        let mut exceptions: Vec<Exception> = ledger
            .map(|l| l.decisions)
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.board_id == board_id)
            .map(|d| Exception {
                key: d.key,
                rule: String::new(),
                subjects: vec![],
                evidence: d.evidence,
                action: if d.action == "ignore" {
                    Action::Ignore
                } else {
                    Action::Flag
                },
                reason: d.reason,
                reviewer: d.reviewer,
                reviewed_at: d.reviewed_at,
                expires_at: d.expires_at,
                severity: String::new(),
                message: String::new(),
                at: None,
                nets: vec![],
            })
            .collect();
        exceptions.sort_by(|a, b| a.key.cmp(&b.key));
        exceptions.dedup_by(|a, b| a.key == b.key);
        BoardLintFile {
            schema: SCHEMA,
            board_id: board_id.into(),
            config: config.unwrap_or_default(),
            exceptions,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            bail!("unsupported lint file schema {}", self.schema)
        }
        if self.board_id.trim().is_empty() {
            bail!("board_id must not be empty")
        }
        self.config.validate()?;
        let mut keys = BTreeSet::new();
        for e in &self.exceptions {
            if e.key.is_empty() || !keys.insert(&e.key) {
                bail!("empty or duplicate exception key {}", e.key)
            }
            if e.reason.trim().is_empty() || e.reviewer.trim().is_empty() {
                bail!("exception {} needs a reason and reviewer", e.key)
            }
        }
        Ok(())
    }
    fn lock_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }
    /// Validate, take `<path>.lock`, and atomically replace the file.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let _guard = review::lock(&Self::lock_path(path))?;
        review::atomic_json(path, self)
    }
    /// Locked read-modify-write: load (or default), mutate, validate, write.
    /// The file is written only when `f` succeeds.
    pub fn update<R>(
        path: &Path,
        default_board_id: &str,
        f: impl FnOnce(&mut BoardLintFile) -> Result<R>,
    ) -> Result<R> {
        let _guard = review::lock(&Self::lock_path(path))?;
        let mut file = Self::load_or_default(path, default_board_id)?;
        let out = f(&mut file)?;
        file.validate()?;
        review::atomic_json(path, &file)?;
        Ok(out)
    }
}

/// Every object identity the board currently defines: tracks, vias, pads,
/// footprints, arcs, zones, keepouts, text, plus `id`s of config-supplied objects
/// (schematic items). Used for orphan detection.
pub fn subject_ids(pcb_source: &str, config: &Config) -> Result<BTreeSet<String>> {
    let b = Board::read(pcb_source)?;
    let x = Extra::read(pcb_source, &b)?;
    let mut ids = BTreeSet::new();
    ids.extend(b.tracks.iter().map(|t| t.id.clone()));
    ids.extend(b.vias.iter().map(|t| t.id.clone()));
    ids.extend(b.pads.iter().map(|t| t.id.clone()));
    ids.extend(b.parts.iter().map(|t| t.id.clone()));
    ids.extend(x.arcs.iter().map(|t| t.id.clone()));
    ids.extend(x.zones.iter().map(|t| t.id.clone()));
    ids.extend(x.keepouts.iter().map(|t| t.id.clone()));
    ids.extend(x.via_keepouts.iter().map(|t| t.id.clone()));
    ids.extend(x.texts.iter().map(|t| t.id.clone()));
    // Config-supplied objects (e.g. schematic items) carry their own ids.
    fn walk(v: &serde_json::Value, ids: &mut BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    if let ("id", serde_json::Value::String(s)) = (k.as_str(), v) {
                        ids.insert(s.clone());
                    }
                    walk(v, ids);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, ids)),
            _ => {}
        }
    }
    walk(&serde_json::to_value(config)?, &mut ids);
    Ok(ids)
}

/// Subjects that name board geometry (contracts are configuration facts).
fn geometric(id: &str) -> bool {
    !id.starts_with("contract:")
}

/// Missing geometric subjects of `e` on a board defining `ids`.
pub fn missing_subjects(e: &Exception, ids: &BTreeSet<String>) -> Vec<String> {
    e.subjects
        .iter()
        .filter(|s| geometric(s) && !ids.contains(*s))
        .cloned()
        .collect()
}

/// Lint with the file's config and audit every exception against the result.
pub fn lint_board(pcb_source: &str, file: &BoardLintFile, at: u64) -> Result<Checked> {
    file.validate()?;
    let mut report = lint(pcb_source, &file.board_id, file.config.clone())?;
    let ids = subject_ids(pcb_source, &file.config)?;
    let coverage: BTreeMap<&str, &str> = report
        .coverage
        .iter()
        .map(|c| (c.rule.as_str(), c.status.as_str()))
        .collect();
    let mut audit = vec![];
    for e in &file.exceptions {
        let finding = report.findings.iter_mut().find(|f| f.key == e.key);
        let expired = e.expires_at.is_some_and(|t| t <= at);
        let (status, detail) = match finding {
            Some(f) => {
                if expired {
                    (
                        ExceptionStatus::Expired,
                        format!("expired at {}", e.expires_at.unwrap_or_default()),
                    )
                } else if !f.stable_identity {
                    (
                        ExceptionStatus::Changed,
                        "finding has no stable identity; review cannot persist".into(),
                    )
                } else if f.evidence != e.evidence {
                    (
                        ExceptionStatus::Changed,
                        "nearby geometry or config changed; finding reopened for review".into(),
                    )
                } else {
                    f.state = match e.action {
                        Action::Ignore => "ignored",
                        Action::Flag => "flagged",
                    }
                    .into();
                    f.review = Some(review::Decision {
                        board_id: file.board_id.clone(),
                        key: e.key.clone(),
                        evidence: e.evidence.clone(),
                        action: e.action.as_str().into(),
                        reason: e.reason.clone(),
                        reviewer: e.reviewer.clone(),
                        reviewed_at: e.reviewed_at,
                        expires_at: e.expires_at,
                    });
                    (ExceptionStatus::Applied, String::new())
                }
            }
            None => {
                let rule_status = coverage.get(e.rule.as_str()).copied();
                let missing = missing_subjects(e, &ids);
                if rule_status != Some("evaluated") {
                    let why = match rule_status {
                        Some(s) => s.to_owned(),
                        None if e.rule.is_empty() => "rule unknown (migrated exception)".into(),
                        None => format!("unknown rule {}", e.rule),
                    };
                    (ExceptionStatus::Unverified, why)
                } else if !missing.is_empty() {
                    (
                        ExceptionStatus::Orphaned,
                        format!(
                            "geometry no longer on board: {}; run kct lint prune",
                            missing.join(", ")
                        ),
                    )
                } else if expired {
                    (
                        ExceptionStatus::Expired,
                        format!("expired at {}", e.expires_at.unwrap_or_default()),
                    )
                } else if e.subjects.is_empty() {
                    (
                        ExceptionStatus::Resolved,
                        "finding no longer raised (no stored subjects)".into(),
                    )
                } else {
                    (
                        ExceptionStatus::Resolved,
                        "finding no longer raised; subjects still on board".into(),
                    )
                }
            }
        };
        report.review_audit.push(review::Audit {
            key: e.key.clone(),
            status: status.as_str().into(),
            reason: e.reason.clone(),
        });
        audit.push(ExceptionAudit {
            key: e.key.clone(),
            rule: e.rule.clone(),
            action: e.action,
            reason: e.reason.clone(),
            reviewer: e.reviewer.clone(),
            status,
            detail,
        });
    }
    Ok(Checked { report, audit })
}

/// Resolve a full key or a unique prefix of at least 8 characters.
fn resolve<'a>(key: &str, keys: impl Iterator<Item = &'a str>) -> Result<Option<&'a str>> {
    let mut hits = vec![];
    for k in keys {
        if k == key {
            return Ok(Some(k));
        }
        if key.len() >= 8 && k.starts_with(key) {
            hits.push(k);
        }
    }
    match hits.len() {
        0 => Ok(None),
        1 => Ok(Some(hits[0])),
        _ => bail!("key prefix {key} is ambiguous"),
    }
}

fn exception_from(f: &Finding, action: Action, reason: &str, reviewer: &str) -> Exception {
    Exception {
        key: f.key.clone(),
        rule: f.rule.clone(),
        subjects: f.subjects.clone(),
        evidence: f.evidence.clone(),
        action,
        reason: reason.into(),
        reviewer: reviewer.into(),
        reviewed_at: review::now(),
        expires_at: None,
        severity: f.severity.clone(),
        message: f.message.clone(),
        at: Some(f.at),
        nets: f.nets.clone(),
    }
}

/// Record or replace an exception for a finding in `report` (full key or unique
/// prefix >= 8 chars). Ignoring a finding without stable identity is refused.
pub fn upsert(
    file: &mut BoardLintFile,
    report: &Report,
    key: &str,
    action: Action,
    reason: &str,
    reviewer: &str,
    expires: Option<u64>,
) -> Result<()> {
    if report.schema != 1 {
        bail!("unsupported report schema")
    }
    if report.board_id != file.board_id {
        bail!(
            "report board_id {} does not match lint file board_id {}",
            report.board_id,
            file.board_id
        )
    }
    let full = resolve(key, report.findings.iter().map(|f| f.key.as_str()))?
        .context("finding key absent from report")?;
    let found = report.findings.iter().find(|f| f.key == full).unwrap();
    if reason.trim().is_empty() || reviewer.trim().is_empty() {
        bail!("reason and reviewer are required")
    }
    if action == Action::Ignore && !found.stable_identity {
        bail!(
            "cannot persistently ignore an object without stable UUID; add UUID or encode intent in configuration"
        )
    }
    if expires.is_some_and(|t| t <= review::now()) {
        bail!("expiry must be in the future")
    }
    let mut e = exception_from(found, action, reason, reviewer);
    e.expires_at = expires;
    file.exceptions.retain(|x| x.key != e.key);
    file.exceptions.push(e);
    file.exceptions.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(())
}

/// Remove an exception by full key or unique prefix. Returns whether one was removed.
pub fn clear(file: &mut BoardLintFile, key: &str) -> bool {
    let Ok(Some(full)) = resolve(key, file.exceptions.iter().map(|e| e.key.as_str())) else {
        return false;
    };
    let full = full.to_owned();
    let before = file.exceptions.len();
    file.exceptions.retain(|e| e.key != full);
    before != file.exceptions.len()
}

/// Fill rule, subjects and card metadata for exceptions (typically migrated)
/// whose finding is raised in `report` but which lack a rule. Returns the count.
pub fn backfill(file: &mut BoardLintFile, report: &Report) -> usize {
    let mut n = 0;
    for e in &mut file.exceptions {
        if !e.rule.is_empty() {
            continue;
        }
        if let Some(f) = report.findings.iter().find(|f| f.key == e.key) {
            let filled = exception_from(f, e.action, &e.reason, &e.reviewer);
            e.rule = filled.rule;
            e.subjects = filled.subjects;
            e.severity = filled.severity;
            e.message = filled.message;
            e.at = filled.at;
            e.nets = filled.nets;
            n += 1;
        }
    }
    n
}

/// Which audit statuses `prune` removes. `unverified` and `applied` are never removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneSet {
    pub orphaned: bool,
    pub resolved: bool,
    pub expired: bool,
    pub changed: bool,
}
impl Default for PruneSet {
    /// Orphaned, resolved and expired; `changed` only on explicit opt-in.
    fn default() -> Self {
        Self {
            orphaned: true,
            resolved: true,
            expired: true,
            changed: false,
        }
    }
}
impl PruneSet {
    pub fn includes(&self, s: ExceptionStatus) -> bool {
        match s {
            ExceptionStatus::Orphaned => self.orphaned,
            ExceptionStatus::Resolved => self.resolved,
            ExceptionStatus::Expired => self.expired,
            ExceptionStatus::Changed => self.changed,
            ExceptionStatus::Applied | ExceptionStatus::Unverified => false,
        }
    }
}

/// Pure in-memory prune; returns the removed exceptions. Exceptions without an
/// audit entry are kept.
pub fn prune(file: &mut BoardLintFile, audit: &[ExceptionAudit], set: PruneSet) -> Vec<Exception> {
    let remove: BTreeSet<&str> = audit
        .iter()
        .filter(|a| set.includes(a.status))
        .map(|a| a.key.as_str())
        .collect();
    let (removed, kept) = std::mem::take(&mut file.exceptions)
        .into_iter()
        .partition(|e| remove.contains(e.key.as_str()));
    file.exceptions = kept;
    removed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    /// Raised after but not before (by finding key).
    pub new: Vec<Finding>,
    /// Raised before but not after.
    pub fixed: Vec<Finding>,
    pub unchanged: usize,
}

/// Compare two reports of the same board by finding key, regardless of state.
/// Gate on `new` (optionally filtered by state/severity) after a routing step.
pub fn diff(before: &Report, after: &Report) -> Delta {
    let b: BTreeSet<&str> = before.findings.iter().map(|f| f.key.as_str()).collect();
    let a: BTreeSet<&str> = after.findings.iter().map(|f| f.key.as_str()).collect();
    Delta {
        new: after
            .findings
            .iter()
            .filter(|f| !b.contains(f.key.as_str()))
            .cloned()
            .collect(),
        fixed: before
            .findings
            .iter()
            .filter(|f| !a.contains(f.key.as_str()))
            .cloned()
            .collect(),
        unchanged: a.intersection(&b).count(),
    }
}

/// Catalog guidance for a rule (used when a stale exception has no message).
pub fn rule_action(rule: &str) -> Option<&'static str> {
    rules::catalog_cached()
        .iter()
        .find(|r| r.id == rule)
        .map(|r| r.action.as_str())
}
