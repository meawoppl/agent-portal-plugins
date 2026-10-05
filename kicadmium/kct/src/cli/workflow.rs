//! Native orchestration for the repair pipeline and spec-driven build.
use super::Globals;
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
struct StepResult {
    name: String,
    args: Vec<String>,
    exit_code: i32,
    skipped: bool,
    reason: Option<String>,
}
pub fn pipeline(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let parsed = PipelineArgs::parse(args)?;
    let board = resolve_board(&parsed.input)?;
    let steps = parsed.step.map_or_else(
        || {
            vec![
                "erc",
                "sync",
                "fix-silkscreen",
                "fix-vias",
                "route",
                "stitch",
                "fix-drc",
                "optimize-traces",
                "zones",
                "audit",
                "report",
                "export",
            ]
        },
        |s| vec![s],
    );
    let mut results = vec![];
    let output_root = parsed
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from("output/kct-pipeline"));
    if !parsed.dry_run {
        std::fs::create_dir_all(&output_root)?;
    }
    for step in steps {
        let Some(command) = step_command(step, &board, &parsed, &output_root) else {
            results.push(StepResult {
                name: step.into(),
                args: vec![],
                exit_code: 0,
                skipped: true,
                reason: Some("native command is not registered yet".into()),
            });
            continue;
        };
        if parsed.dry_run {
            results.push(StepResult {
                name: step.into(),
                args: strings(&command),
                exit_code: 0,
                skipped: true,
                reason: Some("dry run".into()),
            });
            continue;
        }
        let code = crate::cli::run(command.clone())?;
        results.push(StepResult {
            name: step.into(),
            args: strings(&command),
            exit_code: code,
            skipped: false,
            reason: None,
        });
        if code != 0 && !parsed.keep_going {
            emit_results(&parsed.format, &results)?;
            return Ok(code);
        }
    }
    emit_results(&parsed.format, &results)?;
    Ok(if results.iter().any(|r| r.exit_code != 0) {
        1
    } else {
        0
    })
}

struct PipelineArgs {
    input: PathBuf,
    step: Option<&'static str>,
    mfr: String,
    output: Option<PathBuf>,
    dry_run: bool,
    keep_going: bool,
    format: String,
}
impl PipelineArgs {
    fn parse(args: Vec<OsString>) -> Result<Self> {
        let mut input = None;
        let mut step = None;
        let mut mfr = "jlcpcb".to_owned();
        let mut output = None;
        let mut dry_run = false;
        let mut keep_going = false;
        let mut format = "text".to_owned();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.to_str().unwrap_or("") {
                "-s" | "--step" => step = Some(leak(next(&mut it, "--step")?)),
                "-m" | "--mfr" => mfr = next(&mut it, "--mfr")?.to_string_lossy().into_owned(),
                "-o" | "--output" | "--output-dir" => {
                    output = Some(PathBuf::from(next(&mut it, "--output")?))
                }
                "--dry-run" => dry_run = true,
                "--keep-going" => keep_going = true,
                "--format" => format = next(&mut it, "--format")?.to_string_lossy().into_owned(),
                s if s.starts_with('-') => {
                    if flag_takes_value(s) {
                        let _ = it.next();
                    }
                }
                _ => {
                    if input.is_none() {
                        input = Some(PathBuf::from(a))
                    }
                }
            }
        }
        Ok(Self {
            input: input.context("pipeline requires INPUT")?,
            step,
            mfr,
            output,
            dry_run,
            keep_going,
            format,
        })
    }
}
fn flag_takes_value(s: &str) -> bool {
    matches!(
        s,
        "--layers"
            | "--copper"
            | "--strategy"
            | "--jobs"
            | "--timeout"
            | "--route-timeout"
            | "--net-class-map"
            | "--exclude-nets"
    )
}
fn step_command(
    step: &str,
    board: &Path,
    a: &PipelineArgs,
    output_root: &Path,
) -> Option<Vec<OsString>> {
    if crate::cli::COMMANDS
        .iter()
        .find(|spec| spec.name == step)
        .is_some_and(|spec| spec.run.is_none())
    {
        return None;
    }
    let b = board.as_os_str().to_owned();
    let output_board = |name: &str| {
        output_root
            .join(format!("{name}.kicad_pcb"))
            .into_os_string()
    };
    Some(match step {
        "erc" => vec![
            "erc".into(),
            board.with_extension("kicad_sch").into_os_string(),
        ],
        "sync" => vec![
            "sync".into(),
            "--analyze".into(),
            "--schematic".into(),
            board.with_extension("kicad_sch").into_os_string(),
            "--pcb".into(),
            b.clone(),
        ],
        "fix-silkscreen" | "fix-vias" => vec![
            step.into(),
            b,
            "--mfr".into(),
            a.mfr.clone().into(),
            "--output".into(),
            output_board(step),
        ],
        "route" | "optimize-traces" => vec![step.into(), b, "--output".into(), output_board(step)],
        "stitch" => vec!["stitch".into(), b, "--output".into(), output_board(step)],
        "fix-drc" => vec!["fix-drc".into(), b, "--output".into(), output_board(step)],
        "zones" => vec![
            "zones".into(),
            "fill".into(),
            b,
            "--output".into(),
            output_board(step),
        ],
        "audit" => vec![
            step.into(),
            b,
            "--mfr".into(),
            a.mfr.clone().into(),
            "--format".into(),
            "json".into(),
        ],
        "report" => vec![
            "report".into(),
            "generate".into(),
            b,
            "--mfr".into(),
            a.mfr.clone().into(),
            "--output".into(),
            output_root.join("reports").into_os_string(),
            "--format".into(),
            "json".into(),
        ],
        "export" => {
            let mut v = vec!["export".into(), b, "--mfr".into(), a.mfr.clone().into()];
            v.extend([
                "--output".into(),
                output_root.join("manufacturing").into_os_string(),
            ]);
            v
        }
        other => vec![other.into(), b],
    })
}

pub fn build(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let parsed = BuildArgs::parse(args)?;
    let spec: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(&parsed.spec)?)?;
    let base = parsed.spec.parent().unwrap_or(Path::new("."));
    let board = parsed
        .board
        .or_else(|| {
            spec.get("project")
                .and_then(|v| v.get("artifacts"))
                .and_then(|v| v.get("pcb"))
                .and_then(|v| v.as_str())
                .map(|p| base.join(p))
        })
        .or_else(|| find_ext(base, "kicad_pcb"))
        .context("spec does not identify a PCB")?;
    // Upstream's `all` build runs content generation before routing and
    // manufacturing checks. Keep that ordering here: `fix-silkscreen` in the
    // repair pipeline enforces fabrication dimensions, while this distinct
    // step unhides references and adds project markings.
    let selected = match parsed.step {
        None | Some("all") => vec!["silkscreen", "pipeline"],
        Some(step) => vec![step],
    };
    let mut results = vec![];
    for step in selected {
        if step == "silkscreen" {
            let result = run_build_silkscreen(&spec, &board, &parsed.output, parsed.dry_run)?;
            let failed = result.exit_code != 0;
            results.push(result);
            if failed {
                break;
            }
            continue;
        }
        let command = if step == "pipeline" {
            vec![
                "pipeline".into(),
                board.as_os_str().to_owned(),
                "--mfr".into(),
                parsed.mfr.clone().into(),
                "--output".into(),
                parsed.output.as_os_str().to_owned(),
            ]
        } else {
            vec![
                "pipeline".into(),
                board.as_os_str().to_owned(),
                "--step".into(),
                step.into(),
                "--mfr".into(),
                parsed.mfr.clone().into(),
                "--output".into(),
                parsed.output.as_os_str().to_owned(),
            ]
        };
        if parsed.dry_run {
            results.push(StepResult {
                name: step.into(),
                args: strings(&command),
                exit_code: 0,
                skipped: true,
                reason: Some("dry run".into()),
            })
        } else {
            let code = crate::cli::run(command.clone())?;
            results.push(StepResult {
                name: step.into(),
                args: strings(&command),
                exit_code: code,
                skipped: false,
                reason: None,
            });
            if code != 0 {
                break;
            }
        }
    }
    emit_results(&parsed.format, &results)?;
    Ok(if results.iter().any(|r| r.exit_code != 0) {
        1
    } else {
        0
    })
}

fn run_build_silkscreen(
    spec: &serde_yaml::Value,
    board: &Path,
    output_dir: &Path,
    dry_run: bool,
) -> Result<StepResult> {
    let output = output_dir.join("silkscreen.kicad_pcb");
    let args = vec![
        board.display().to_string(),
        "--output".into(),
        output.display().to_string(),
    ];
    if dry_run {
        return Ok(StepResult {
            name: "silkscreen".into(),
            args,
            exit_code: 0,
            skipped: true,
            reason: Some("dry run".into()),
        });
    }

    std::fs::create_dir_all(output_dir)?;
    std::fs::copy(board, &output).with_context(|| {
        format!(
            "failed to stage {} for silkscreen generation",
            board.display()
        )
    })?;
    let mut generator = crate::silkscreen::generator::SilkscreenGenerator::new(&output)?;
    let refs = generator.ensure_ref_des_visible();
    let project = spec.get("project");
    let name = project
        .and_then(|p| p.get("name"))
        .and_then(serde_yaml::Value::as_str);
    let revision = project
        .and_then(|p| p.get("revision"))
        .and_then(serde_yaml::Value::as_str);
    let created = project
        .and_then(|p| p.get("created"))
        .and_then(yaml_scalar_text);
    let markings =
        generator.add_board_markings(name, revision, created.as_deref(), "F.SilkS", 1.0, 0.15);
    generator.save(Some(&output))?;

    let changes = refs.total_changes() + markings.total_changes();
    Ok(StepResult {
        name: "silkscreen".into(),
        args,
        exit_code: 0,
        skipped: false,
        reason: Some(format!(
            "{changes} change(s): {} reference(s) unhidden, {} marking(s) added, {} already present",
            refs.refs_unhidden, markings.markings_added, markings.markings_skipped
        )),
    })
}

fn yaml_scalar_text(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
struct BuildArgs {
    spec: PathBuf,
    board: Option<PathBuf>,
    step: Option<&'static str>,
    mfr: String,
    output: PathBuf,
    dry_run: bool,
    format: String,
}
impl BuildArgs {
    fn parse(args: Vec<OsString>) -> Result<Self> {
        let mut spec = None;
        let mut board = None;
        let mut step = None;
        let mut mfr = "jlcpcb".into();
        let mut output = PathBuf::from("output");
        let mut dry_run = false;
        let mut format = "text".into();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.to_str().unwrap_or("") {
                "--board" => board = Some(PathBuf::from(next(&mut it, "--board")?)),
                "-s" | "--step" => step = Some(leak(next(&mut it, "--step")?)),
                "-m" | "--mfr" => mfr = next(&mut it, "--mfr")?.to_string_lossy().into_owned(),
                "-o" | "--output" | "--output-dir" => {
                    output = PathBuf::from(next(&mut it, "--output")?)
                }
                "--dry-run" => dry_run = true,
                "--format" => format = next(&mut it, "--format")?.to_string_lossy().into_owned(),
                s if s.starts_with('-') => {
                    if flag_takes_value(s) {
                        let _ = it.next();
                    }
                }
                _ => {
                    if spec.is_none() {
                        spec = Some(PathBuf::from(a))
                    }
                }
            }
        }
        Ok(Self {
            spec: spec.context("build requires SPEC.kct")?,
            board,
            step,
            mfr,
            output,
            dry_run,
            format,
        })
    }
}
fn resolve_board(p: &Path) -> Result<PathBuf> {
    if p.extension().and_then(OsStr::to_str) == Some("kicad_pcb") {
        return Ok(p.into());
    }
    let base = p.parent().unwrap_or(Path::new("."));
    let direct = p.with_extension("kicad_pcb");
    if direct.exists() {
        Ok(direct)
    } else {
        find_ext(base, "kicad_pcb").context("no PCB found")
    }
}
fn find_ext(dir: &Path, ext: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().and_then(OsStr::to_str) == Some(ext))
}
fn next(it: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString> {
    it.next()
        .with_context(|| format!("{flag} requires a value"))
}
fn leak(v: OsString) -> &'static str {
    Box::leak(v.to_string_lossy().into_owned().into_boxed_str())
}
fn strings(v: &[OsString]) -> Vec<String> {
    v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
}
fn emit_results(format: &str, r: &[StepResult]) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(r)?)
    } else {
        for x in r {
            println!(
                "{:<18} {}{}",
                x.name,
                if x.skipped {
                    "SKIPPED"
                } else if x.exit_code == 0 {
                    "OK"
                } else {
                    "FAIL"
                },
                if x.exit_code == 0 {
                    x.reason
                        .as_ref()
                        .map_or_else(String::new, |reason| format!(" ({reason})"))
                } else {
                    format!(" ({})", x.exit_code)
                }
            )
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn silkscreen_fixture() -> &'static str {
        r#"(kicad_pcb (version 20240108) (generator "test")
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (37 "F.SilkS" user "f.silkscreen"))
  (footprint "R:0805" (at 10 10) (layer "F.Cu")
    (fp_text reference "R1" (at 0 -2) (layer "F.SilkS") hide
      (effects (font (size 1 1) (thickness 0.15)))))
)"#
    }

    #[test]
    fn parses_pipeline() {
        let a = PipelineArgs::parse(
            ["x.kicad_pcb", "--step", "audit", "--dry-run"]
                .into_iter()
                .map(Into::into)
                .collect(),
        )
        .unwrap();
        assert_eq!(a.step, Some("audit"));
        assert!(a.dry_run)
    }

    #[test]
    fn workflow_argv_uses_real_subcommands_and_outputs() {
        let args = PipelineArgs {
            input: "board.kicad_pcb".into(),
            step: None,
            mfr: "jlcpcb".into(),
            output: Some("artifacts".into()),
            dry_run: true,
            keep_going: false,
            format: "json".into(),
        };
        let board = Path::new("board.kicad_pcb");
        let root = Path::new("artifacts");
        let zones = strings(&step_command("zones", board, &args, root).unwrap());
        assert_eq!(zones[0..2], ["zones", "fill"]);
        assert!(zones
            .windows(2)
            .any(|v| v == ["--output", "artifacts/zones.kicad_pcb"]));
        let report = strings(&step_command("report", board, &args, root).unwrap());
        assert_eq!(report[0..2], ["report", "generate"]);
        let sync = strings(&step_command("sync", board, &args, root).unwrap());
        assert!(sync.contains(&"--analyze".to_string()));
        assert!(sync.contains(&"--schematic".to_string()));
        assert!(sync.contains(&"--pcb".to_string()));
    }

    #[test]
    fn build_silkscreen_stages_output_and_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("source.kicad_pcb");
        let output = dir.path().join("artifacts");
        std::fs::write(&board, silkscreen_fixture()).unwrap();
        let original = std::fs::read(&board).unwrap();
        let spec: serde_yaml::Value = serde_yaml::from_str(
            "project:\n  name: Toxic Uncle\n  revision: A\n  created: 2026-10-01\n",
        )
        .unwrap();

        let result = run_build_silkscreen(&spec, &board, &output, false).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(!result.skipped);
        assert_eq!(std::fs::read(&board).unwrap(), original);
        let generated = std::fs::read_to_string(output.join("silkscreen.kicad_pcb")).unwrap();
        assert!(generated.contains("Toxic Uncle Rev A"), "{generated}");
        assert!(!generated.contains("F.SilkS\") hide"), "{generated}");
        assert!(output.join("silkscreen.kicad_pcb.kct.json").exists());
    }

    #[test]
    fn build_silkscreen_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("source.kicad_pcb");
        let output = dir.path().join("artifacts");
        std::fs::write(&board, silkscreen_fixture()).unwrap();
        let spec: serde_yaml::Value = serde_yaml::from_str("project:\n  name: Preview\n").unwrap();

        let result = run_build_silkscreen(&spec, &board, &output, true).unwrap();

        assert!(result.skipped);
        assert!(!output.exists());
    }
}
