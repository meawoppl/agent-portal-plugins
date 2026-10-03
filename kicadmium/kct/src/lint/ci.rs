//! Pure CI renderers over a [`Checked`] run: a pass/fail summary, a GitHub
//! step-summary markdown table and SARIF 2.1.0. `write_bundle` is an optional
//! convenience that writes the standard artifact set for one board.
use crate::lint::{
    board_file::{BoardLintFile, Checked, ExceptionAudit, ExceptionStatus},
    contact_sheet, review, rules,
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fmt::Write, path::Path, str::FromStr};

pub const SEVERITIES: [&str; 3] = ["error", "warning", "info"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailOn {
    #[default]
    Never,
    Warning,
    Error,
}
impl FromStr for FailOn {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "never" => Self::Never,
            "warning" => Self::Warning,
            "error" => Self::Error,
            _ => bail!("expected never, warning or error"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiSummary {
    pub board_id: String,
    /// Findings not ignored by an applied exception (flagged findings count), by severity.
    pub open_by_severity: BTreeMap<String, usize>,
    pub ignored: usize,
    pub flagged: usize,
    pub exceptions_by_status: BTreeMap<String, usize>,
    /// Orphaned, resolved and expired exceptions.
    pub stale_exceptions: Vec<ExceptionAudit>,
}
impl From<&Checked> for CiSummary {
    fn from(c: &Checked) -> Self {
        let mut open_by_severity: BTreeMap<String, usize> =
            SEVERITIES.iter().map(|s| (s.to_string(), 0)).collect();
        let mut ignored = 0;
        let mut flagged = 0;
        for f in &c.report.findings {
            match f.state.as_str() {
                "ignored" => ignored += 1,
                s => {
                    flagged += usize::from(s == "flagged");
                    *open_by_severity.entry(f.severity.clone()).or_default() += 1;
                }
            }
        }
        let mut exceptions_by_status: BTreeMap<String, usize> = ExceptionStatus::ALL
            .iter()
            .map(|s| (s.as_str().to_owned(), 0))
            .collect();
        for a in &c.audit {
            *exceptions_by_status
                .entry(a.status.as_str().to_owned())
                .or_default() += 1;
        }
        Self {
            board_id: c.report.board_id.clone(),
            open_by_severity,
            ignored,
            flagged,
            exceptions_by_status,
            stale_exceptions: c.stale().cloned().collect(),
        }
    }
}
impl CiSummary {
    pub fn open(&self, severity: &str) -> usize {
        self.open_by_severity.get(severity).copied().unwrap_or(0)
    }
    /// False when an open finding meets the threshold. Stale exceptions warn but never fail.
    pub fn passed(&self, fail_on: FailOn) -> bool {
        match fail_on {
            FailOn::Never => true,
            FailOn::Error => self.open("error") == 0,
            FailOn::Warning => self.open("error") + self.open("warning") == 0,
        }
    }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Compact GitHub-step-summary markdown. `links` are `(label, href)` pairs,
/// typically the bundle's findings.html / exceptions.html / report.json / SARIF.
pub fn summary_markdown(checked: &Checked, file: &BoardLintFile, links: &[(&str, &str)]) -> String {
    let s = CiSummary::from(checked);
    let mut md = String::new();
    let _ = writeln!(md, "### pcb-lint · `{}`\n", cell(&file.board_id));
    let open: usize = s.open_by_severity.values().sum();
    let _ = writeln!(md, "| Open findings | error | warning | info | total |");
    let _ = writeln!(md, "|---|---:|---:|---:|---:|");
    let _ = writeln!(
        md,
        "| {} | {} | {} | {} | {open} |\n",
        if open == 0 { "none" } else { "open" },
        s.open("error"),
        s.open("warning"),
        s.open("info"),
    );
    let _ = writeln!(
        md,
        "{} ignored by exceptions · {} flagged (still open)\n",
        s.ignored, s.flagged
    );
    let _ = write!(md, "| Exceptions |");
    for st in ExceptionStatus::ALL {
        let _ = write!(md, " {st} |");
    }
    let _ = write!(md, " total |\n|---|");
    for _ in ExceptionStatus::ALL {
        md.push_str("---:|");
    }
    md.push_str("---:|\n| count |");
    for st in ExceptionStatus::ALL {
        let _ = write!(
            md,
            " {} |",
            s.exceptions_by_status.get(st.as_str()).unwrap_or(&0)
        );
    }
    let _ = writeln!(md, " {} |\n", file.exceptions.len());
    if s.stale_exceptions.is_empty() {
        md.push_str("No stale exceptions.\n");
    } else {
        let _ = writeln!(
            md,
            "**{} stale exception{}** (run `kct lint prune`):\n",
            s.stale_exceptions.len(),
            if s.stale_exceptions.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        md.push_str("| status | rule | key | detail |\n|---|---|---|---|\n");
        for a in &s.stale_exceptions {
            let _ = writeln!(
                md,
                "| {} | `{}` | `{}` | {} |",
                a.status,
                cell(&a.rule),
                &a.key[..a.key.len().min(12)],
                cell(&a.detail)
            );
        }
    }
    if !links.is_empty() {
        md.push('\n');
        let parts: Vec<_> = links
            .iter()
            .map(|(label, href)| format!("[{}]({})", cell(label), href.replace(' ', "%20")))
            .collect();
        let _ = writeln!(md, "{}", parts.join(" · "));
    }
    md
}

fn level(severity: &str) -> &'static str {
    match severity {
        "error" => "error",
        "warning" => "warning",
        _ => "note",
    }
}

/// SARIF 2.1.0 log for one or more boards. `ruleId` is the lint rule, `level`
/// follows severity, the physical location is the board file and a logical
/// location carries board coordinates; fingerprints are the finding key.
/// Findings ignored by an applied exception are emitted as suppressed results.
pub fn sarif(boards: &[(&Path, &Checked)]) -> Value {
    let catalog = rules::catalog_cached();
    let mut used = std::collections::BTreeSet::new();
    let mut results = vec![];
    for (path, c) in boards {
        let uri = path.to_string_lossy().replace('\\', "/");
        for f in &c.report.findings {
            used.insert(f.rule.clone());
            let mut text = f.message.clone();
            if !f.suggestion.is_empty() {
                text = format!("{text}. {}", f.suggestion);
            }
            let mut r = json!({
                "ruleId": f.rule,
                "level": level(&f.severity),
                "message": {"text": text},
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": {"uri": uri},
                        "region": {"startLine": 1}
                    },
                    "logicalLocations": [{
                        "name": format!("({:.3}, {:.3}) mm", f.at.x, f.at.y),
                        "fullyQualifiedName": format!("{}@{:.3},{:.3}", c.report.board_id, f.at.x, f.at.y),
                        "kind": "element"
                    }]
                }],
                "fingerprints": {"pcbLintKey/v1": f.key},
                "partialFingerprints": {"pcbLintKey/v1": f.key},
                "properties": {
                    "board_id": c.report.board_id,
                    "state": f.state,
                    "confidence": f.confidence,
                    "subjects": f.subjects,
                    "nets": f.nets,
                    "at_mm": {"x": f.at.x, "y": f.at.y},
                    "evidence": f.evidence
                }
            });
            if let Some(d) = f.review.as_ref().filter(|_| f.state == "ignored") {
                r["suppressions"] = json!([{
                    "kind": "external",
                    "status": "accepted",
                    "justification": format!("{} ({})", d.reason, d.reviewer)
                }]);
            }
            results.push(r);
        }
    }
    let rules: Vec<Value> = catalog
        .iter()
        .filter(|r| used.contains(&r.id))
        .map(|r| {
            json!({
                "id": r.id,
                "shortDescription": {"text": r.detection},
                "help": {"text": r.action},
                "defaultConfiguration": {"level": level(&r.severity)},
                "properties": {"family": r.family, "confidence": r.confidence, "caveat": r.caveat}
            })
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {"driver": {
                "name": "pcb-lint",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://github.com/meawoppl/kicadmium",
                "rules": rules
            }},
            "results": results
        }]
    })
}

/// Convenience: write report.json, findings.html, exceptions.html, summary.md
/// and findings.sarif for one board into `out_dir`.
pub fn write_bundle(
    out_dir: &Path,
    board_path: &Path,
    pcb_source: &str,
    checked: &Checked,
    file: &BoardLintFile,
) -> Result<CiSummary> {
    std::fs::create_dir_all(out_dir)?;
    review::atomic_json(&out_dir.join("report.json"), &checked.report)?;
    let findings = contact_sheet::render(pcb_source, &checked.report)?;
    review::atomic_write(&out_dir.join("findings.html"), findings.as_bytes())?;
    let exceptions =
        contact_sheet::render_exceptions(pcb_source, &checked.report, file, &checked.audit)?;
    review::atomic_write(&out_dir.join("exceptions.html"), exceptions.as_bytes())?;
    review::atomic_json(
        &out_dir.join("findings.sarif"),
        &sarif(&[(board_path, checked)]),
    )?;
    let md = summary_markdown(
        checked,
        file,
        &[
            ("findings", "findings.html"),
            ("exceptions", "exceptions.html"),
            ("report.json", "report.json"),
            ("SARIF", "findings.sarif"),
        ],
    );
    review::atomic_write(&out_dir.join("summary.md"), md.as_bytes())?;
    Ok(CiSummary::from(checked))
}
