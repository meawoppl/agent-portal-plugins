use crate::lint::{corpus, lint, review, rules, Config, Report};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
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
    corpus::read_config(p)
}
fn protect(output: Option<&Path>, inputs: &[&Path]) -> Result<()> {
    review::protect_output(output, inputs)
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
            let m = corpus::Manifest::load(&manifest)?;
            let base = manifest.parent().unwrap_or(Path::new("."));
            for input in m.inputs(base) {
                protect(p.as_deref(), &[input.as_path()])?;
            }
            let results = m.evaluate(base)?;
            let failed = results.iter().any(|r| !r.passed);
            output(&results, p.as_deref())?;
            Ok(if failed { 2 } else { 0 })
        }
    }
}
