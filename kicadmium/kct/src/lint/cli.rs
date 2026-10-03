use crate::lint::{lint, review, rules, Config, Report};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};
#[derive(Parser)]
#[command(
    version,
    about = "Read-only KiCad geometry lint with evidence-bound review decisions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run read-only heuristic checks and emit a versioned JSON report.
    Lint {
        /// Canonical .kicad_pcb input. This command never modifies it.
        board: PathBuf,
        /// Stable project/board identity included in evidence keys.
        #[arg(long)]
        board_id: String,
        /// JSON Config. Omitted fields use the values printed by init-config.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Evidence-bound review ledger; stale evidence is not reused.
        #[arg(long)]
        reviews: Option<PathBuf>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Exit 2 when an open finding meets this severity threshold.
        #[arg(long, value_enum, default_value = "never")]
        fail_on: FailOn,
        /// Write a self-contained HTML contact sheet with highlighted vector crops.
        #[arg(long, value_name = "HTML")]
        contact_sheet: Option<PathBuf>,
    },
    /// Parse a board and emit the normalized geometry model as JSON.
    Inspect {
        board: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Emit the complete rule catalog as JSON.
    Rules,
    /// Emit the complete default JSON configuration, including numeric defaults.
    InitConfig {
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Add, flag, clear, or expire an evidence-bound review decision.
    Review {
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        ledger: PathBuf,
        #[arg(long)]
        key: String,
        #[arg(value_enum)]
        action: Action,
        #[arg(long, default_value = "")]
        reason: String,
        #[arg(long, default_value = "")]
        reviewer: String,
        #[arg(long)]
        expires_at: Option<u64>,
    },
    /// Run a deny-unknown-fields JSON corpus manifest and check expectations.
    Corpus {
        manifest: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}
#[derive(Clone, ValueEnum)]
enum Action {
    Ignore,
    Flag,
    Clear,
}
#[derive(Clone, ValueEnum)]
enum FailOn {
    Never,
    Warning,
    Error,
}
fn config(p: Option<&Path>) -> Result<Config> {
    Ok(if let Some(p) = p {
        serde_json::from_str(
            &fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
        )?
    } else {
        Config::default()
    })
}
fn protect(output: Option<&Path>, inputs: &[&Path]) -> Result<()> {
    if let Some(out) = output {
        if out
            .extension()
            .is_some_and(|e| e == "kicad_pcb" || e == "kicad_sch" || e == "kicad_pro")
        {
            bail!("refusing to overwrite a KiCad design with JSON")
        }
        let identity = |p: &Path| -> Result<PathBuf> {
            if p.exists() {
                Ok(p.canonicalize()?)
            } else {
                let abs = if p.is_absolute() {
                    p.to_owned()
                } else {
                    std::env::current_dir()?.join(p)
                };
                Ok(abs)
            }
        };
        let target = identity(out)?;
        for input in inputs {
            if target == identity(input)? {
                bail!("output aliases an input: {}", input.display())
            }
        }
    }
    Ok(())
}
fn output<T: Serialize>(v: &T, path: Option<&Path>) -> Result<()> {
    if let Some(p) = path {
        protect(Some(p), &[])?;
        review::atomic_json(p, v)
    } else {
        println!("{}", serde_json::to_string_pretty(v)?);
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    board: PathBuf,
    board_id: String,
    config: Option<PathBuf>,
    expected: Vec<Expectation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expectation {
    rule: String,
    min: usize,
    max: usize,
    #[serde(default)]
    subjects: Vec<String>,
}
#[derive(Serialize)]
struct Evaluation {
    name: String,
    passed: bool,
    counts: BTreeMap<String, usize>,
    mismatches: Vec<String>,
    error: Option<String>,
}
/// Run the complete pcb-lint CLI inside another Rust binary.
///
/// `args` contains subcommand arguments only; a synthetic executable name is
/// supplied here so embedded callers cannot accidentally change parsing.
pub fn run<I>(args: I) -> Result<i32>
where
    I: IntoIterator<Item = OsString>,
{
    match Cli::parse_from(std::iter::once(OsString::from("pcb-lint")).chain(args)).command {
        Command::Inspect { board, output: p } => {
            protect(p.as_deref(), &[&board])?;
            output(
                &crate::lint::model::Board::read(&fs::read_to_string(board)?)?,
                p.as_deref(),
            )?;
            Ok(0)
        }
        Command::Rules => {
            output(&rules::catalog(), None)?;
            Ok(0)
        }
        Command::InitConfig { output: p } => {
            output(&Config::default(), p.as_deref())?;
            Ok(0)
        }
        Command::Lint {
            board,
            board_id,
            config: cp,
            reviews,
            output: p,
            fail_on,
            contact_sheet,
        } => {
            let mut inputs = vec![board.as_path()];
            inputs.extend(cp.as_deref());
            inputs.extend(reviews.as_deref());
            protect(p.as_deref(), &inputs)?;
            if let Some(sheet) = &contact_sheet {
                if sheet.extension().is_none_or(|e| e != "html") {
                    bail!("contact sheet output must end in .html")
                }
                protect(Some(sheet), &inputs)?;
                if let Some(report) = &p {
                    protect(Some(sheet), &[report])?;
                }
            }
            let input = fs::read_to_string(&board)
                .with_context(|| format!("reading {}", board.display()))?;
            let mut r = lint(&input, &board_id, config(cp.as_deref())?)?;
            if let Some(path) = reviews {
                review::apply(&mut r, &review::load(&path)?, review::now());
            }
            let open = r.findings.iter().filter(|f| f.state != "ignored").count();
            let coverage_states =
                r.coverage
                    .iter()
                    .fold(BTreeMap::new(), |mut counts, coverage| {
                        *counts.entry(coverage.status.as_str()).or_insert(0usize) += 1;
                        counts
                    });
            let coverage_summary = coverage_states
                .iter()
                .map(|(state, count)| format!("{state}={count}"))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "{} findings: {} actionable, {} ignored; {} coverage entries ({coverage_summary}). Heuristics are not DRC.",
                r.findings.len(),
                open,
                r.findings.len() - open,
                r.coverage.len()
            );
            let fail = r.findings.iter().any(|f| {
                f.state != "ignored"
                    && match fail_on {
                        FailOn::Never => false,
                        FailOn::Warning => f.severity == "warning" || f.severity == "error",
                        FailOn::Error => f.severity == "error",
                    }
            });
            if let Some(sheet) = contact_sheet {
                let html = crate::lint::contact_sheet::render(&input, &r)?;
                review::atomic_write(&sheet, html.as_bytes())?;
                eprintln!("Contact sheet: {}", sheet.display());
            }
            output(&r, p.as_deref())?;
            Ok(if fail { 2 } else { 0 })
        }
        Command::Review {
            report,
            ledger,
            key,
            action,
            reason,
            reviewer,
            expires_at,
        } => {
            protect(Some(&ledger), &[&report])?;
            let r: Report = serde_json::from_str(&fs::read_to_string(report)?)?;
            let action = match action {
                Action::Ignore => "ignore",
                Action::Flag => "flag",
                Action::Clear => "clear",
            };
            review::update(&ledger, &r, &key, action, &reason, &reviewer, expires_at)?;
            eprintln!(
                "{action}: {key}; rerun lint with --reviews {}",
                ledger.display()
            );
            Ok(0)
        }
        Command::Corpus {
            manifest,
            output: p,
        } => {
            protect(p.as_deref(), &[&manifest])?;
            let m: Manifest = serde_json::from_str(&fs::read_to_string(&manifest)?)?;
            if m.schema != 1 {
                bail!("unsupported corpus schema")
            }
            if m.cases.is_empty() {
                bail!("corpus must have at least one case")
            }
            let base = manifest.parent().unwrap_or(Path::new("."));
            for case in &m.cases {
                protect(p.as_deref(), &[&base.join(&case.board)])?;
                if let Some(cp) = &case.config {
                    protect(p.as_deref(), &[&base.join(cp)])?;
                }
            }
            let catalog = rules::catalog();
            let mut names = std::collections::BTreeSet::new();
            let mut results = vec![];
            for case in m.cases {
                if !names.insert(case.name.clone()) || case.expected.is_empty() {
                    bail!("case names must be unique and each case needs expectations")
                }
                for x in &case.expected {
                    if x.min > x.max
                        || !catalog
                            .iter()
                            .any(|r| r.id == x.rule && r.status == "implemented")
                    {
                        bail!("invalid expectation {}", x.rule)
                    }
                }
                let result = (|| {
                    let text = fs::read_to_string(base.join(&case.board))?;
                    lint(
                        &text,
                        &case.board_id,
                        config(case.config.as_ref().map(|p| base.join(p)).as_deref())?,
                    )
                })();
                match result {
                    Ok(r) => {
                        let mut counts = BTreeMap::new();
                        for f in &r.findings {
                            *counts.entry(f.rule.clone()).or_default() += 1;
                        }
                        let mut mismatches = vec![];
                        for x in case.expected {
                            if !r
                                .coverage
                                .iter()
                                .any(|c| c.rule == x.rule && c.status == "evaluated")
                            {
                                mismatches.push(format!("{} not evaluated", x.rule));
                                continue;
                            }
                            let n = r
                                .findings
                                .iter()
                                .filter(|f| {
                                    f.rule == x.rule
                                        && x.subjects.iter().all(|s| f.subjects.contains(s))
                                })
                                .count();
                            if n < x.min || n > x.max {
                                mismatches.push(format!(
                                    "{}: found {n}, expected {}..={} on {:?}",
                                    x.rule, x.min, x.max, x.subjects
                                ));
                            }
                        }
                        results.push(Evaluation {
                            name: case.name,
                            passed: mismatches.is_empty(),
                            counts,
                            mismatches,
                            error: None,
                        });
                    }
                    Err(err) => results.push(Evaluation {
                        name: case.name,
                        passed: false,
                        counts: BTreeMap::new(),
                        mismatches: vec![],
                        error: Some(format!("{err:#}")),
                    }),
                }
            }
            let failed = results.iter().any(|r| !r.passed);
            output(&results, p.as_deref())?;
            Ok(if failed { 2 } else { 0 })
        }
    }
}
