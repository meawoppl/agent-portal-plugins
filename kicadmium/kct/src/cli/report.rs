use super::{parse_args, Globals};
use crate::{cost, schema::pcb::Pcb};
use anyhow::{bail, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde_json::json;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

#[derive(serde::Serialize)]
struct CheckEvidence {
    status: &'static str,
    errors: Option<usize>,
    warnings: Option<usize>,
    detail: String,
}

fn count_severities(value: &serde_json::Value, errors: &mut usize, warnings: &mut usize) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                count_severities(item, errors, warnings);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(severity) = map.get("severity").and_then(serde_json::Value::as_str) {
                match severity.to_ascii_lowercase().as_str() {
                    "error" => *errors += 1,
                    "warning" => *warnings += 1,
                    _ => {}
                }
            }
            for child in map.values() {
                if !child.is_string() {
                    count_severities(child, errors, warnings);
                }
            }
        }
        _ => {}
    }
}

fn native_check(kind: &str, input: &Path) -> CheckEvidence {
    let Some(cli) = crate::cli::runner::find_kicad_cli() else {
        return CheckEvidence {
            status: "unknown",
            errors: None,
            warnings: None,
            detail: "kicad-cli not found".into(),
        };
    };
    let report = std::env::temp_dir().join(format!(
        "kct-report-{kind}-{}-{}.json",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let output = ProcessCommand::new(cli)
        .args([
            kind,
            if kind == "pcb" { "drc" } else { "erc" },
            "--format",
            "json",
            "-o",
        ])
        .arg(&report)
        .arg(input)
        .output();
    let parsed = std::fs::read(&report)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let _ = std::fs::remove_file(report);
    let Some(value) = parsed else {
        return CheckEvidence {
            status: "unknown",
            errors: None,
            warnings: None,
            detail: output
                .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_owned())
                .unwrap_or_else(|e| e.to_string()),
        };
    };
    let mut errors = 0;
    let mut warnings = 0;
    count_severities(&value, &mut errors, &mut warnings);
    CheckEvidence {
        status: "measured",
        errors: Some(errors),
        warnings: Some(warnings),
        detail: "measured with kicad-cli JSON report".into(),
    }
}

fn status_line(evidence: &CheckEvidence) -> String {
    match (evidence.errors, evidence.warnings) {
        (Some(errors), Some(warnings)) => format!("{errors} error(s), {warnings} warning(s)"),
        _ => format!("{} ({})", evidence.status, evidence.detail),
    }
}
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Generate(Generate),
}
#[derive(ClapArgs)]
struct Generate {
    /// Board to report on (.kicad_pcb, or .kicad_pro resolved by stem)
    input: PathBuf,
    /// Target manufacturer (default: unknown)
    #[arg(long, default_value = "unknown")]
    mfr: String,
    /// Output directory for versioned reports (default: reports/)
    #[arg(short, long, default_value = "reports")]
    output: PathBuf,
    /// Directory containing pre-collected data snapshots (drc_summary.json,
    /// erc_summary.json, notes.txt, metadata.json); used instead of running
    /// kicad-cli, and figures are not generated
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Path to a custom template file (not supported natively)
    #[arg(long)]
    template: Option<PathBuf>,
    /// Path to root .kicad_sch file (inferred from input if omitted)
    #[arg(long)]
    sch: Option<PathBuf>,
    /// Skip figure generation
    #[arg(long)]
    no_figures: bool,
    /// Batch quantity for the cost estimate
    #[arg(long, default_value_t = 5)]
    quantity: usize,
    /// Skip ERC collection
    #[arg(long)]
    skip_erc: bool,
    /// Skip data collection entirely (skeleton report)
    #[arg(long)]
    skip_collect: bool,
    /// Output format (text or json)
    #[arg(long, default_value = "text")]
    format: String,
}

/// Pre-collected `--data-dir` snapshots (upstream `_load_data_dir`).
#[derive(Default)]
struct DataDir {
    drc: Option<serde_json::Value>,
    erc: Option<serde_json::Value>,
    notes: Option<String>,
    revision: Option<String>,
    date: Option<String>,
}

/// Collector envelope `{"schema_version", "data"}` -> `data`; flat JSON is
/// returned unchanged; a `data: null` envelope (failed collector) is `None`.
fn unwrap_envelope(v: serde_json::Value) -> Option<serde_json::Value> {
    match v {
        serde_json::Value::Object(ref m)
            if m.contains_key("schema_version") && m.contains_key("data") =>
        {
            Some(m["data"].clone()).filter(|d| !d.is_null())
        }
        other => Some(other),
    }
}

fn load_data_dir(dir: &Path) -> Result<DataDir> {
    if !dir.is_dir() {
        eprintln!(
            "WARNING: --data-dir {} does not exist; DRC/ERC evidence will be reported as unknown",
            dir.display()
        );
        return Ok(DataDir::default());
    }
    let json = |name: &str| -> Result<Option<serde_json::Value>> {
        let path = dir.join(name);
        if !path.is_file() {
            return Ok(None);
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)
            .map_err(|e| anyhow::anyhow!("{}: invalid JSON: {e}", path.display()))?;
        Ok(unwrap_envelope(v))
    };
    let meta = json("metadata.json")?;
    let meta_str = |k: &str| {
        meta.as_ref().and_then(|m| m.get(k)).map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        })
    };
    Ok(DataDir {
        drc: json("drc_summary.json")?,
        erc: json("erc_summary.json")?,
        notes: std::fs::read_to_string(dir.join("notes.txt"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        revision: meta_str("revision"),
        date: meta_str("date"),
    })
}

/// Evidence from a pre-collected `*_summary.json` (`error_count`/`warning_count`).
fn data_dir_evidence(summary: Option<&serde_json::Value>, file: &str) -> CheckEvidence {
    let count = |k: &str| {
        summary
            .and_then(|s| s.get(k))
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as usize)
    };
    match (count("error_count"), count("warning_count")) {
        (Some(errors), Some(warnings)) => CheckEvidence {
            status: "pre-collected",
            errors: Some(errors),
            warnings: Some(warnings),
            detail: format!("pre-collected {file} from --data-dir"),
        },
        _ => CheckEvidence {
            status: "unknown",
            errors: None,
            warnings: None,
            detail: format!("no usable {file} in --data-dir"),
        },
    }
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let Args {
        command: Command::Generate(a),
    } = parse_args::<Args>("report", args);
    if a.template.is_some() {
        bail!("--template is not supported by the native report generator")
    }
    let pcb = if a.input.extension().and_then(|x| x.to_str()) == Some("kicad_pro") {
        a.input.with_extension("kicad_pcb")
    } else {
        a.input.clone()
    };
    if !pcb.is_file() {
        bail!("PCB file not found: {}", pcb.display())
    }
    let p = Pcb::load(&pcb)?;
    let s = p.summary()?;
    let c = cost::estimate(&p, &a.mfr, a.quantity, "hasl", "green", 1.6);
    let version = next_version(&a.output);
    std::fs::create_dir_all(&version)?;
    let report = version.join("report.md");
    let project = pcb.file_stem().unwrap_or_default().to_string_lossy();
    let schematic = a.sch.clone().or_else(|| {
        let candidate = pcb.with_extension("kicad_sch");
        candidate.exists().then_some(candidate)
    });
    if a.sch.as_ref().is_some_and(|path| !path.exists()) {
        bail!("schematic not found: {}", a.sch.as_ref().unwrap().display())
    }
    let data = match &a.data_dir {
        Some(dir) => Some(load_data_dir(dir)?),
        None => None,
    };
    let drc = if let Some(d) = &data {
        data_dir_evidence(d.drc.as_ref(), "drc_summary.json")
    } else if a.skip_collect {
        CheckEvidence {
            status: "skipped",
            errors: None,
            warnings: None,
            detail: "--skip-collect".into(),
        }
    } else {
        native_check("pcb", &pcb)
    };
    let erc = if let Some(d) = &data {
        data_dir_evidence(d.erc.as_ref(), "erc_summary.json")
    } else if a.skip_collect || a.skip_erc {
        CheckEvidence {
            status: "skipped",
            errors: None,
            warnings: None,
            detail: if a.skip_collect {
                "--skip-collect"
            } else {
                "--skip-erc"
            }
            .into(),
        }
    } else if let Some(path) = schematic.as_deref() {
        native_check("sch", path)
    } else {
        CheckEvidence {
            status: "unknown",
            errors: None,
            warnings: None,
            detail: "no schematic discovered".into(),
        }
    };
    let completion = if s.nets > 0 {
        "not measured"
    } else {
        "not applicable (no nets)"
    };
    let md=format!("---\ntitle: \"{} Design Report\"\n---\n\n# {}\n\n## Board Summary\n\n| Metric | Value |\n|---|---|\n| Board Size | {:.2} x {:.2} mm |\n| Layers | {} copper |\n| Footprints | {} |\n| Signal Net Completion | {} |\n\n## Verification Evidence\n\n| Check | Result |\n|---|---|\n| DRC | {} |\n| ERC | {} |\n\nUnknown or skipped checks are not passes.\n\n## Cost Estimate\n\nTotal (estimated): ~{:.2} USD\n\n| Batch Quantity | {} |\n\nBatch Total (estimated): ~{:.2} USD\n",project,project,s.width_mm,s.height_mm,s.copper_layers,s.footprints,completion,status_line(&drc),status_line(&erc),c["summary"]["total_per_unit"].as_f64().unwrap_or(0.),a.quantity,c["summary"]["total_for_quantity"].as_f64().unwrap_or(0.));
    let mut md = md;
    if let Some(d) = &data {
        if d.revision.is_some() || d.date.is_some() {
            md.push_str(&format!(
                "\n## Revision\n\n| Revision | {} |\n|---|---|\n| Date | {} |\n",
                d.revision.as_deref().unwrap_or("-"),
                d.date.as_deref().unwrap_or("-")
            ));
        }
        if let Some(notes) = &d.notes {
            md.push_str(&format!("\n## Notes\n\n{notes}\n"));
        }
    }
    crate::fsutil::atomic_write(&report, md.as_bytes())?;
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"command":"generate","input":a.input,"manufacturer":a.mfr,"output_dir":a.output,"project_name":project,"report_path":report,"pdf_path":null,"data_source":if data.is_some(){"data-dir"}else if a.skip_collect{"skipped"}else{"native-kicad-cli"},"drc":drc,"erc":erc,"schematic":schematic,"figures":{"generated":false,"skipped_reason":if a.no_figures{"disabled by --no-figures"}else if data.is_some(){"pre-collected via --data-dir"}else{"run kct render explicitly; report generation does not fabricate figures"}},"success":true})
            )?
        )
    } else {
        println!("Report written to {}", report.display())
    }
    Ok(0)
}
fn next_version(root: &Path) -> PathBuf {
    let mut n = 1;
    while root.join(format!("v{n}")).exists() {
        n += 1
    }
    root.join(format!("v{n}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versioning() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(next_version(d.path()), d.path().join("v1"));
        std::fs::create_dir(d.path().join("v1")).unwrap();
        assert_eq!(next_version(d.path()), d.path().join("v2"));
    }
}
