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
    input: PathBuf,
    #[arg(long, default_value = "unknown")]
    mfr: String,
    #[arg(short, long, default_value = "reports")]
    output: PathBuf,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    template: Option<PathBuf>,
    #[arg(long)]
    sch: Option<PathBuf>,
    #[arg(long)]
    no_figures: bool,
    #[arg(long, default_value_t = 5)]
    quantity: usize,
    #[arg(long)]
    skip_erc: bool,
    #[arg(long)]
    skip_collect: bool,
    #[arg(long, default_value = "text")]
    format: String,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let Args {
        command: Command::Generate(a),
    } = parse_args::<Args>("report", args);
    if a.data_dir.is_some() {
        bail!("--data-dir is not supported by the native report generator")
    }
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
    let drc = if a.skip_collect {
        CheckEvidence {
            status: "skipped",
            errors: None,
            warnings: None,
            detail: "--skip-collect".into(),
        }
    } else {
        native_check("pcb", &pcb)
    };
    let erc = if a.skip_collect || a.skip_erc {
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
    crate::fsutil::atomic_write(&report, md.as_bytes())?;
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"command":"generate","input":a.input,"manufacturer":a.mfr,"output_dir":a.output,"project_name":project,"report_path":report,"pdf_path":null,"data_source":if a.skip_collect{"skipped"}else{"native-kicad-cli"},"drc":drc,"erc":erc,"schematic":schematic,"figures":{"generated":false,"skipped_reason":if a.no_figures{"disabled by --no-figures"}else{"run kct render explicitly; report generation does not fabricate figures"}},"success":true})
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
