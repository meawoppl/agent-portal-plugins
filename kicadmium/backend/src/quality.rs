//! Layout-quality stage: kicad-tools rules plus in-plugin audits, normalized
//! into one report with severities taken from the workspace quality profile.
//!
//! kct integration is generic: every rule `kct check` reports is passed
//! through (except the families that duplicate native KiCad DRC), so new
//! upstream rules show up after a kct upgrade without plugin changes. Rule ids
//! are mapped to profile items by substring so the profile can set their
//! severity. In-plugin audits (see `audits.rs`) skip themselves when kct
//! reports an equivalent rule, and can be disabled wholesale.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shared::CheckResponse;

use crate::{
    audits,
    jobs::Outcome,
    profile::{Level, Profile},
    revision, sexpr, ProjectContext,
};

/// Bumped when audit logic changes so cached quality results rebuild.
pub(crate) const AUDIT_VERSION: &str = "quality/v1";

/// `quality` section of `.kicad-pcb.json`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct QualityConfig {
    /// Run kicad-tools rules (default true).
    pub kct: Option<bool>,
    /// Run in-plugin audits (default true). Set false once kct covers them.
    pub plugin_audits: Option<bool>,
    /// In-plugin audit ids to skip.
    pub disabled_audits: Option<Vec<String>>,
    /// Keep kct rules that duplicate native KiCad DRC (default false).
    pub include_drc_rules: Option<bool>,
    /// Run `kct detect-mistakes` (default true).
    pub detect_mistakes: Option<bool>,
    /// Report `kct optimize-traces --dry-run` savings (default true).
    pub optimize_traces: Option<bool>,
    /// Severity overrides keyed by rule id or profile item
    /// (`error`, `warning`, `info`, `off`).
    pub severity: Option<BTreeMap<String, String>>,
}

pub(crate) fn validate_config(config: &QualityConfig) -> Result<()> {
    for id in config.disabled_audits.iter().flatten() {
        if !audits::AUDITS.iter().any(|spec| spec.id == id) {
            return Err(anyhow!(
                "quality.disabledAudits has unknown audit {id:?}; expected one of {}",
                audits::AUDITS
                    .iter()
                    .map(|spec| spec.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    for (key, value) in config.severity.iter().flatten() {
        if Level::parse(value).is_none() {
            return Err(anyhow!(
                "quality.severity.{key} must be error, warning, info, or off"
            ));
        }
    }
    Ok(())
}

/// Resolved per-project quality settings.
#[derive(Debug, Clone, Default)]
pub(crate) struct QualitySettings {
    /// Absolute path to the profile YAML, if configured.
    pub profile: Option<PathBuf>,
    pub config: QualityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Pos {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct FindingItem {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos: Option<Pos>,
}

/// One normalized layout-quality finding. Field names match the DRC report
/// shape the checks tab already renders (`type`, `severity`, `description`,
/// `items`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Finding {
    #[serde(rename = "type")]
    pub rule: String,
    pub severity: String,
    pub description: String,
    /// `plugin`, `kct check`, `kct detect-mistakes`, `kct optimize-traces`.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_item: Option<String>,
    pub items: Vec<FindingItem>,
    #[serde(skip)]
    default: Option<Level>,
}

impl Finding {
    pub(crate) fn new(
        rule: &str,
        default: Level,
        description: String,
        source: &str,
        items: Vec<FindingItem>,
    ) -> Self {
        Self {
            rule: rule.to_string(),
            severity: default.as_str().to_string(),
            description,
            source: source.to_string(),
            profile_item: None,
            items,
            default: Some(default),
        }
    }
}

// ---------------------------------------------------------------------------
// kct rule mapping

/// kct rule families that duplicate native KiCad DRC (which runs with the
/// board's own `.kicad_dru`) or schematic checks covered by ERC.
const DRC_DUPLICATE_PREFIXES: &[&str] = &[
    "clearance",
    "dimension_",
    "min_",
    "hole_",
    "edge_",
    "annular",
    "drill",
    "courtyard",
    "connectivity",
    "diffpair_clearance",
    "copper_sliver",
    "single_pad_net",
    "solder_mask",
    "mask_",
    "sch_field",
    "doc_drift",
];

/// Substring of a kct rule id -> profile item that sets its severity.
const RULE_PROFILE_MAP: &[(&str, &str)] = &[
    ("via_in_pad", "vias.in_pad"),
    ("via_in_smd", "vias.in_pad"),
    ("via_under", "vias.under_package_body"),
    ("orphan", "vias.orphans"),
    ("via_dangling", "vias.orphans"),
    ("redundant_ground", "vias.redundant_ground"),
    ("redundant_via", "vias.redundant_ground"),
    ("track_dangling", "routing.stubs"),
    ("stub", "routing.stubs"),
    ("isolated_copper", "routing.stubs"),
    ("width_island", "routing.width_islands"),
    ("width_consistency", "routing.width_islands"),
    (
        "width_transition",
        "routing.width_transition_requires_clearance_reason",
    ),
    ("pad_graz", "routing.pad_grazing"),
    ("off_axis", "routing.octilinear_only"),
    ("octilinear", "routing.octilinear_only"),
    ("pad_exit", "routing.straight_pad_exits"),
    ("silk_over", "silkscreen.over_vias_or_pads"),
    ("silkscreen_over", "silkscreen.over_vias_or_pads"),
    ("pin1", "silkscreen.pin1_dots"),
    ("pin_1", "silkscreen.pin1_dots"),
    ("silkscreen_text", "silkscreen.text_height_mm"),
    ("thermal_relief", "planes.thermal_reliefs"),
    ("decoupl", "decoupling.max_pin_distance_mm"),
    ("bypass", "decoupling.max_pin_distance_mm"),
    ("cpl", "manufacturing.cpl_offsets"),
];

pub(crate) fn is_drc_duplicate(rule: &str) -> bool {
    DRC_DUPLICATE_PREFIXES
        .iter()
        .any(|prefix| rule.starts_with(prefix))
}

pub(crate) fn profile_item_for(rule: &str) -> Option<&'static str> {
    RULE_PROFILE_MAP
        .iter()
        .find(|(pattern, _)| rule.contains(pattern))
        .map(|(_, item)| *item)
}

fn level_from_kct(value: Option<&str>) -> Level {
    value.and_then(Level::parse).unwrap_or(Level::Warning)
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// Parses JSON from a kct stdout that may carry progress lines before it.
pub(crate) fn parse_kct_json(stdout: &str) -> Option<Value> {
    let start = stdout.find('{')?;
    serde_json::from_str(&stdout[start..]).ok()
}

/// Normalized `kct check --format json` output.
#[derive(Debug, Default)]
pub(crate) struct KctCheck {
    pub findings: Vec<Finding>,
    /// Every rule id kct reports as checked or violated.
    pub rules: HashSet<String>,
    pub excluded: BTreeMap<String, usize>,
    pub routing_quality: Option<Value>,
}

pub(crate) fn normalize_kct_check(report: &Value, include_drc: bool) -> KctCheck {
    let mut out = KctCheck {
        routing_quality: report.get("routing_quality").cloned(),
        ..KctCheck::default()
    };
    if let Some(checked) = report
        .pointer("/summary/rules_checked_by_rule")
        .and_then(Value::as_object)
    {
        out.rules.extend(checked.keys().cloned());
    }
    for violation in report
        .get("violations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(rule) = violation
            .get("rule_id")
            .or_else(|| violation.get("type"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        out.rules.insert(rule.to_string());
        if violation.get("waived").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if !include_drc && is_drc_duplicate(rule) {
            *out.excluded.entry(rule.to_string()).or_default() += 1;
            continue;
        }
        let strings = |key: &str| -> Vec<String> {
            violation
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        };
        let mut label = strings("items").join(", ");
        let nets = strings("nets");
        if !nets.is_empty() {
            label = format!("{label} [{}]", nets.join(", "));
        }
        if let Some(layer) = violation.get("layer").and_then(Value::as_str) {
            label = format!("{label} on {layer}");
        }
        let pos = violation
            .get("location")
            .and_then(Value::as_array)
            .and_then(|loc| {
                Some(Pos {
                    x: loc.first()?.as_f64()?,
                    y: loc.get(1)?.as_f64()?,
                })
            });
        let mut finding = Finding::new(
            rule,
            level_from_kct(violation.get("severity").and_then(Value::as_str)),
            violation
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(rule)
                .to_string(),
            "kct check",
            vec![FindingItem {
                description: label.trim().to_string(),
                pos,
            }],
        );
        finding.profile_item = profile_item_for(rule).map(str::to_string);
        out.findings.push(finding);
    }
    out
}

/// Normalized `kct detect-mistakes --format json` output. Rule ids are
/// `mistake.<category>.<title slug>`.
pub(crate) fn normalize_detect_mistakes(
    report: &Value,
    rules: &mut HashSet<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for check in report
        .get("coverage")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(category) = check.get("category").and_then(Value::as_str) {
            rules.insert(format!("mistake.{category}"));
        }
    }
    for mistake in report
        .get("mistakes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let category = mistake
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("other");
        let title = mistake
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("mistake");
        let rule = format!("mistake.{category}.{}", slug(title));
        rules.insert(rule.clone());
        let components: Vec<&str> = mistake
            .get("components")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let explanation = mistake
            .get("explanation")
            .and_then(Value::as_str)
            .unwrap_or(title);
        let mut finding = Finding::new(
            &rule,
            level_from_kct(mistake.get("severity").and_then(Value::as_str)),
            format!("{title}: {explanation}"),
            "kct detect-mistakes",
            vec![FindingItem {
                // detect-mistakes locations are relative to the board origin,
                // not sheet coordinates, so only the parts are reported.
                description: components.join(", "),
                pos: None,
            }],
        );
        finding.profile_item = profile_item_for(&rule).map(str::to_string);
        out.push(finding);
    }
    out
}

pub(crate) fn normalize_optimize_traces(report: &Value) -> Option<Finding> {
    let stats = report.get("stats")?;
    let before = stats.get("segments_before")?.as_u64()?;
    let after = stats.get("segments_after")?.as_u64()?;
    if after >= before {
        return None;
    }
    let corners = |key: &str| stats.get(key).and_then(Value::as_u64).unwrap_or(0);
    let length = stats
        .get("length_reduction_pct")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some(Finding::new(
        "optimize_traces.mergeable_segments",
        Level::Info,
        format!(
            "kct optimize-traces would remove {} of {before} segments (corners {} -> {}, length -{length:.1}%); merge collinear segments and drop zigzags",
            before - after,
            corners("corners_before"),
            corners("corners_after"),
        ),
        "kct optimize-traces",
        Vec::new(),
    ))
}

/// Applies config and profile severities; returns kept findings and the
/// number suppressed (`off`).
pub(crate) fn apply_levels(
    findings: Vec<Finding>,
    profile: &Profile,
    config: &QualityConfig,
) -> (Vec<Finding>, usize) {
    let overrides = config.severity.clone().unwrap_or_default();
    let mut kept = Vec::new();
    let mut suppressed = 0;
    for mut finding in findings {
        let from_config = overrides
            .get(&finding.rule)
            .or_else(|| {
                finding
                    .profile_item
                    .as_ref()
                    .and_then(|item| overrides.get(item))
            })
            .and_then(|value| Level::parse(value));
        let from_profile = finding
            .profile_item
            .as_deref()
            .and_then(|item| profile.level(item));
        let level = from_config
            .or(from_profile)
            .or(finding.default)
            .unwrap_or(Level::Warning);
        if level == Level::Off {
            suppressed += 1;
            continue;
        }
        finding.severity = level.as_str().to_string();
        kept.push(finding);
    }
    (kept, suppressed)
}

// ---------------------------------------------------------------------------
// Stage

pub(crate) fn profile_sha(project: &ProjectContext) -> String {
    project
        .quality
        .profile
        .as_deref()
        .and_then(revision::file_sha256)
        .unwrap_or_default()
}

struct KctRun {
    command: Vec<String>,
    code: i32,
    json: Option<Value>,
    stderr: String,
}

async fn run_kct(kct: &str, args: Vec<OsString>, cwd: &Path) -> KctRun {
    let command = std::iter::once("kct".to_string())
        .chain(args.iter().map(|arg| arg.to_string_lossy().into_owned()))
        .collect();
    // `kct` is this binary's own subcommand; run it out of process so stdout
    // (the JSON report) is captured per invocation.
    let argv: Vec<OsString> = ["kct".into(), "--".into()]
        .into_iter()
        .chain(args)
        .collect();
    match crate::jobs::run_command(kct, &argv, cwd).await {
        Ok(output) => KctRun {
            command,
            code: output.status,
            json: parse_kct_json(&output.stdout),
            stderr: output.stderr,
        },
        Err(err) => KctRun {
            command,
            code: -1,
            json: None,
            stderr: format!("{err:#}"),
        },
    }
}

fn kct_log(run: &KctRun) -> Value {
    json!({
        "command": run.command,
        "code": run.code,
        "parsed": run.json.is_some(),
        "stderr": crate::jobs::tail_text(&run.stderr, 1200),
    })
}

fn sorted(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut list: Vec<String> = items.collect();
    list.sort();
    list
}

/// Runs the full quality pass for a project (uncached).
pub(crate) async fn run_check(project: &ProjectContext) -> Result<CheckResponse> {
    let Some(pcb) = crate::pick_project_file(project, "kicad_pcb")? else {
        return Ok(CheckResponse {
            ok: false,
            source: None,
            command: vec![],
            stdout: String::new(),
            stderr: String::new(),
            report: String::new(),
            message: Some(
                "Select a single .kicad_pcb or set kicad.pcb in .kicad-pcb.json".to_string(),
            ),
            tool: None,
        });
    };
    let config = &project.quality.config;
    let mut notes = Vec::new();
    let profile = match &project.quality.profile {
        Some(path) => Profile::load(path)?,
        None => {
            notes.push(
                "No qualityProfile configured in .kicad-pcb.json; using built-in defaults."
                    .to_string(),
            );
            Profile::default()
        }
    };

    let mut findings = Vec::new();
    let mut rules = HashSet::new();
    let mut excluded = BTreeMap::new();
    let mut routing_quality = None;
    let mut logs = Vec::new();
    let kct = crate::kct_cli();
    let kct_version = crate::kct_version();
    if config.kct.unwrap_or(true) {
        match &kct {
            None => {
                notes.push("kct entry point unavailable; only in-house audits ran.".to_string())
            }
            Some(kct) => {
                let mut check_args: Vec<OsString> = vec![
                    "check".into(),
                    pcb.as_os_str().into(),
                    "--format".into(),
                    "json".into(),
                    "--drc-only".into(),
                ];
                if let Some(fab) = profile.str("manufacturing.fab") {
                    check_args.push("--mfr".into());
                    check_args.push(fab.into());
                }
                let mistakes_args: Vec<OsString> = vec![
                    "-q".into(),
                    "detect-mistakes".into(),
                    pcb.as_os_str().into(),
                    "--format".into(),
                    "json".into(),
                ];
                // optimize-traces rewrites its input unless --dry-run works;
                // run it on a scratch copy regardless.
                let scratch = tempfile::TempDir::new()?;
                let copy = scratch.path().join("board.kicad_pcb");
                tokio::fs::copy(&pcb, &copy).await.context("copy board")?;
                let optimize_args: Vec<OsString> = vec![
                    "optimize-traces".into(),
                    copy.as_os_str().into(),
                    "--dry-run".into(),
                    "--format".into(),
                    "json".into(),
                ];
                let run_mistakes = config.detect_mistakes.unwrap_or(true);
                let run_optimize = config.optimize_traces.unwrap_or(true);
                let (check, mistakes, optimize) = tokio::join!(
                    run_kct(kct, check_args, &project.root),
                    async {
                        if run_mistakes {
                            Some(run_kct(kct, mistakes_args, &project.root).await)
                        } else {
                            None
                        }
                    },
                    async {
                        if run_optimize {
                            Some(run_kct(kct, optimize_args, scratch.path()).await)
                        } else {
                            None
                        }
                    },
                );
                for run in [Some(&check), mistakes.as_ref(), optimize.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    if run.json.is_none() {
                        notes.push(format!(
                            "`{}` produced no JSON (exit {}); its rules are missing from this report.",
                            run.command[1..].join(" "),
                            run.code
                        ));
                    }
                    logs.push(kct_log(run));
                }
                if let Some(report) = &check.json {
                    let normalized =
                        normalize_kct_check(report, config.include_drc_rules.unwrap_or(false));
                    findings.extend(normalized.findings);
                    rules.extend(normalized.rules);
                    excluded = normalized.excluded;
                    routing_quality = normalized.routing_quality;
                }
                if let Some(report) = mistakes.as_ref().and_then(|run| run.json.as_ref()) {
                    findings.extend(normalize_detect_mistakes(report, &mut rules));
                }
                if let Some(finding) = optimize
                    .as_ref()
                    .and_then(|run| run.json.as_ref())
                    .and_then(normalize_optimize_traces)
                {
                    findings.push(finding);
                }
            }
        }
    }

    let mut ran = Vec::new();
    let mut superseded = BTreeMap::new();
    let disabled: HashSet<&str> = config
        .disabled_audits
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    if config.plugin_audits.unwrap_or(true) {
        let mut skip: HashSet<&'static str> = HashSet::new();
        for spec in audits::AUDITS {
            if disabled.contains(spec.id) {
                skip.insert(spec.id);
            } else if let Some(rule) = spec
                .kct_equivalents
                .iter()
                .find(|rule| rules.contains(**rule))
            {
                skip.insert(spec.id);
                superseded.insert(spec.id, *rule);
            } else {
                ran.push(spec.id);
            }
        }
        let text = tokio::fs::read_to_string(&pcb)
            .await
            .with_context(|| format!("read {}", pcb.display()))?;
        let profile_for_audits = profile.clone();
        let audit_findings = tokio::task::spawn_blocking(move || -> Result<Vec<Finding>> {
            let root = sexpr::parse(&text)?;
            let board = audits::parse_board(&root);
            Ok(audits::run(&board, &profile_for_audits, &skip))
        })
        .await??;
        findings.extend(audit_findings);
    }

    let (mut findings, suppressed) = apply_levels(findings, &profile, config);
    let rank = |severity: &str| match severity {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    };
    findings.sort_by(|a, b| {
        rank(&a.severity)
            .cmp(&rank(&b.severity))
            .then_with(|| a.rule.cmp(&b.rule))
    });
    let mut by_rule: BTreeMap<String, Value> = BTreeMap::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for finding in &findings {
        *counts.entry(finding.severity.as_str()).or_default() += 1;
        let entry = by_rule.entry(finding.rule.clone()).or_insert_with(|| {
            json!({
                "count": 0,
                "severity": finding.severity,
                "source": finding.source,
                "profile_item": finding.profile_item,
            })
        });
        entry["count"] = json!(entry["count"].as_u64().unwrap_or(0) + 1);
    }
    let errors = counts.get("error").copied().unwrap_or(0);
    let warnings = counts.get("warning").copied().unwrap_or(0);
    let infos = counts.get("info").copied().unwrap_or(0);
    let source = crate::rel(&project.root, &pcb)?;
    let profile_rel = project.quality.profile.as_ref().map(|path| {
        crate::rel(&project.repo_root, path).unwrap_or_else(|_| path.display().to_string())
    });
    let report = json!({
        "version": 1,
        "generator": "kicadmium quality",
        "audit_version": AUDIT_VERSION,
        "source": source,
        "profile": {
            "path": profile_rel,
            "name": profile.name,
            "sha256": profile_sha(project),
        },
        "kct": {
            "tool": kct,
            "version": kct_version,
            "commands": logs,
            "rules_seen": sorted(rules.iter().cloned()),
            "excluded_drc_duplicates": excluded,
        },
        "audits": {
            "enabled": config.plugin_audits.unwrap_or(true),
            "ran": ran,
            "superseded_by_kct": superseded,
            "disabled": sorted(disabled.iter().map(|id| id.to_string())),
        },
        "summary": {
            "errors": errors,
            "warnings": warnings,
            "infos": infos,
            "suppressed": suppressed,
            "by_rule": by_rule,
        },
        "routing_quality": routing_quality,
        "notes": notes,
        "violations": findings,
    });
    let mut stdout = vec![format!(
        "{errors} errors, {warnings} warnings, {infos} info across {} rules",
        by_rule.len()
    )];
    stdout.extend(notes.iter().cloned());
    Ok(CheckResponse {
        ok: errors == 0,
        source: Some(source),
        command: logs
            .iter()
            .filter_map(|log| log["command"].as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect(),
        stdout: stdout.join("\n"),
        stderr: String::new(),
        report: serde_json::to_string_pretty(&report)?,
        message: None,
        tool: kct,
    })
}

/// Build-stage entry point: writes `quality.json` and `check.json`.
pub(crate) async fn execute(project: &ProjectContext, out: &Path) -> Result<Outcome> {
    let check = run_check(project).await?;
    if !check.report.is_empty() {
        tokio::fs::write(out.join("quality.json"), &check.report).await?;
    }
    tokio::fs::write(out.join("check.json"), serde_json::to_vec_pretty(&check)?).await?;
    let summary: Value = serde_json::from_str(&check.report).unwrap_or(Value::Null);
    let count = |key: &str| {
        summary
            .pointer(&format!("/summary/{key}"))
            .and_then(Value::as_u64)
    };
    let message = match (count("errors"), count("warnings")) {
        (Some(errors), Some(warnings)) => Some(format!("{errors} errors, {warnings} warnings")),
        _ => check.message.clone(),
    };
    Ok(Outcome {
        ok: check.ok,
        message,
        log: check.stdout.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kct_check_passes_unknown_rules_and_drops_drc_duplicates() {
        let report = json!({
            "summary": {"rules_checked_by_rule": {"track_dangling": 1, "via_under_package": 1}},
            "violations": [
                {"rule_id": "clearance_segment_via", "severity": "error", "message": "c"},
                {"rule_id": "silk_over_copper", "severity": "warning", "message": "silk",
                 "location": [1.0, 2.0], "items": ["J3 (reference)"], "layer": "F.SilkS"},
                {"rule_id": "brand_new_rule", "severity": "info", "message": "new"},
                {"rule_id": "via_under_package", "severity": "error", "message": "w", "waived": true}
            ]
        });
        let out = normalize_kct_check(&report, false);
        let rules: Vec<_> = out.findings.iter().map(|f| f.rule.as_str()).collect();
        assert_eq!(rules, ["silk_over_copper", "brand_new_rule"]);
        assert_eq!(out.excluded.get("clearance_segment_via"), Some(&1));
        assert!(out.rules.contains("via_under_package"));
        assert_eq!(
            out.findings[0].profile_item.as_deref(),
            Some("silkscreen.over_vias_or_pads")
        );
        assert_eq!(out.findings[0].items[0].pos, Some(Pos { x: 1.0, y: 2.0 }));
        assert_eq!(normalize_kct_check(&report, true).findings.len(), 3);
    }

    #[test]
    fn detect_mistakes_rule_ids_map_to_profile() {
        let report = json!({
            "coverage": [{"category": "via_placement"}],
            "mistakes": [{"category": "via_placement", "severity": "warning",
                          "title": "Via in SMD pad", "components": ["R20", "pad 2"],
                          "explanation": "wicks"}]
        });
        let mut rules = HashSet::new();
        let found = normalize_detect_mistakes(&report, &mut rules);
        assert_eq!(found[0].rule, "mistake.via_placement.via_in_smd_pad");
        assert_eq!(found[0].profile_item.as_deref(), Some("vias.in_pad"));
        assert!(rules.contains("mistake.via_placement"));
    }

    #[test]
    fn levels_follow_config_then_profile_then_default() {
        let profile = Profile::parse("vias:\n  in_pad: forbid\n  orphans: false\n").unwrap();
        let mut a = Finding::new("x", Level::Warning, "a".into(), "kct check", vec![]);
        a.profile_item = Some("vias.in_pad".into());
        let mut b = Finding::new("orphan_via", Level::Error, "b".into(), "plugin", vec![]);
        b.profile_item = Some("vias.orphans".into());
        let c = Finding::new("other", Level::Info, "c".into(), "plugin", vec![]);
        let d = Finding::new("loud", Level::Error, "d".into(), "plugin", vec![]);
        let config = QualityConfig {
            severity: Some([("loud".to_string(), "info".to_string())].into()),
            ..QualityConfig::default()
        };
        let (kept, suppressed) = apply_levels(vec![a, b, c, d], &profile, &config);
        assert_eq!(suppressed, 1);
        let levels: Vec<_> = kept
            .iter()
            .map(|f| (f.rule.as_str(), f.severity.as_str()))
            .collect();
        assert_eq!(
            levels,
            [("x", "error"), ("other", "info"), ("loud", "info")]
        );
    }

    #[test]
    fn optimize_traces_reports_savings() {
        let report = json!({"stats": {"segments_before": 647, "segments_after": 592,
            "corners_before": 498, "corners_after": 483, "length_reduction_pct": 0.47}});
        let finding = normalize_optimize_traces(&report).unwrap();
        assert!(finding.description.contains("remove 55 of 647"));
        let same = json!({"stats": {"segments_before": 5, "segments_after": 5}});
        assert!(normalize_optimize_traces(&same).is_none());
    }

    #[test]
    fn kct_json_skips_progress_lines() {
        assert_eq!(
            parse_kct_json("Analyzing: a.kicad_pcb\n{\"a\": 1}"),
            Some(json!({"a": 1}))
        );
        assert!(parse_kct_json("no json").is_none());
    }

    #[test]
    fn config_validation() {
        let bad = QualityConfig {
            disabled_audits: Some(vec!["nope".into()]),
            ..QualityConfig::default()
        };
        assert!(validate_config(&bad).is_err());
        let bad = QualityConfig {
            severity: Some([("x".to_string(), "loud".to_string())].into()),
            ..QualityConfig::default()
        };
        assert!(validate_config(&bad).is_err());
        let good: QualityConfig = serde_json::from_str(
            r#"{"pluginAudits": true, "disabledAudits": ["orphan_via"], "severity": {"vias.in_pad": "warn"}}"#,
        )
        .unwrap();
        assert!(validate_config(&good).is_ok());
        assert!(serde_json::from_str::<QualityConfig>(r#"{"bogus": 1}"#).is_err());
    }
}
