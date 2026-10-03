//! `kct lint`: canonical evidence-aware PCB lint workflow.

use super::{parse_args, Globals};
use crate::lint::{
    board_file::{self, Action, BoardLintFile, ExceptionStatus, PruneSet},
    review, rules,
};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run lint using the board's adjacent .lint.json policy file.
    Run(RunArgs),
    Init(InitArgs),
    Waive(DecisionArgs),
    Flag(DecisionArgs),
    Clear(KeyArgs),
    Stale(BoardArgs),
    Prune(PruneArgs),
    Diff(DiffArgs),
    Rules(FormatArgs),
    Ci(CiArgs),
    /// Parse a board and dump its normalized geometry model (json) or object
    /// counts (text). Use it to find UUIDs and exact net/reference names.
    Inspect(InspectArgs),
    /// Evaluate a labelled corpus manifest against expected finding counts.
    /// Exits 2 when any case fails.
    Corpus(CorpusArgs),
}
#[derive(Debug, Clone, Args)]
struct BoardArgs {
    board: PathBuf,
    /// Override the adjacent <board>.lint.json policy path.
    #[arg(long)]
    file: Option<PathBuf>,
}
#[derive(Debug, Clone, Args)]
struct RunArgs {
    #[command(flatten)]
    board: BoardArgs,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    #[arg(long, value_enum, default_value_t = FailOn::Never)]
    fail_on: FailOn,
    #[arg(long)]
    contact_sheet: Option<PathBuf>,
    /// Write the exception audit as a self-contained HTML contact sheet.
    #[arg(long)]
    exceptions_sheet: Option<PathBuf>,
}
#[derive(Debug, Clone, Args)]
struct InitArgs {
    #[arg(required_unless_present = "print_default_config")]
    board: Option<PathBuf>,
    #[arg(long)]
    board_id: Option<String>,
    #[arg(long)]
    force: bool,
    /// Print the complete default lint config (every threshold and intent
    /// field) as JSON instead of creating a lint file.
    #[arg(long, conflicts_with_all = ["board", "board_id", "force"])]
    print_default_config: bool,
}
#[derive(Debug, Clone, Args)]
struct InspectArgs {
    board: PathBuf,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Write to this file instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
}
#[derive(Debug, Clone, Args)]
struct CorpusArgs {
    /// Corpus manifest (schema 1); case paths resolve relative to it.
    manifest: PathBuf,
    /// Write the evaluation JSON to this file instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
}
#[derive(Debug, Clone, Args)]
struct DecisionArgs {
    #[command(flatten)]
    board: BoardArgs,
    key: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    reviewer: String,
    #[arg(long)]
    expires_at: Option<String>,
}
#[derive(Debug, Clone, Args)]
struct KeyArgs {
    #[command(flatten)]
    board: BoardArgs,
    key: String,
}
#[derive(Debug, Clone, Args)]
struct PruneArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Write the change. Without this flag, only show what would be removed.
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    changed: bool,
    #[arg(long)]
    keep_orphaned: bool,
    #[arg(long)]
    keep_resolved: bool,
    #[arg(long)]
    keep_expired: bool,
}
#[derive(Debug, Clone, Args)]
struct DiffArgs {
    before: PathBuf,
    after: PathBuf,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}
#[derive(Debug, Clone, Args)]
struct FormatArgs {
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}
#[derive(Debug, Clone, Args)]
struct CiArgs {
    #[arg(required = true)]
    boards: Vec<PathBuf>,
    #[arg(long, default_value = "lint-results")]
    out: PathBuf,
    #[arg(long, value_enum, default_value_t = FailOn::Error)]
    fail_on: FailOn,
}
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
}
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum FailOn {
    Never,
    Warning,
    Error,
}

#[derive(Serialize)]
struct RunOutput<'a> {
    board: &'a Path,
    policy: &'a Path,
    report: &'a crate::lint::Report,
    exceptions: &'a [board_file::ExceptionAudit],
}

fn board_id(board: &Path) -> String {
    board
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board")
        .to_owned()
}
fn load(args: &BoardArgs) -> Result<(String, PathBuf, BoardLintFile, board_file::Checked)> {
    let source = fs::read_to_string(&args.board)
        .with_context(|| format!("reading {}", args.board.display()))?;
    let path = policy_path(args);
    let file = BoardLintFile::load_or_default(&path, &board_id(&args.board))?;
    let checked = board_file::lint_board(&source, &file, review::now())?;
    Ok((source, path, file, checked))
}
fn warn_audit(checked: &board_file::Checked) {
    for a in &checked.audit {
        if a.status != ExceptionStatus::Applied {
            eprintln!("lint exception {} is {}: {}", a.key, a.status, a.detail);
        }
    }
}
fn open_at(report: &crate::lint::Report, threshold: FailOn) -> usize {
    report
        .findings
        .iter()
        .filter(|f| {
            f.state != "ignored"
                && match threshold {
                    FailOn::Never => false,
                    FailOn::Warning => matches!(f.severity.as_str(), "warning" | "error"),
                    FailOn::Error => f.severity == "error",
                }
        })
        .count()
}
/// Sheets must not overwrite the board, its lint file, or each other.
fn protect_sheets(args: &RunArgs) -> Result<()> {
    let policy = policy_path(&args.board);
    let inputs = [args.board.board.as_path(), policy.as_path()];
    review::protect_output(args.contact_sheet.as_deref(), &inputs)?;
    review::protect_output(args.exceptions_sheet.as_deref(), &inputs)?;
    if let (Some(findings), Some(exceptions)) = (&args.contact_sheet, &args.exceptions_sheet) {
        review::protect_output(Some(exceptions), &[findings])?;
    }
    Ok(())
}
fn policy_path(args: &BoardArgs) -> PathBuf {
    args.file
        .clone()
        .unwrap_or_else(|| BoardLintFile::path_for(&args.board))
}
/// Write `text` to `path` (refusing to alias `inputs`), or print it.
fn emit(text: &str, path: Option<&Path>, inputs: &[&Path]) -> Result<()> {
    match path {
        Some(path) => {
            review::protect_output(Some(path), inputs)?;
            review::atomic_write(path, text.as_bytes())
        }
        None => {
            println!("{text}");
            Ok(())
        }
    }
}
fn inspect(args: InspectArgs) -> Result<i32> {
    review::protect_output(args.output.as_deref(), &[&args.board])?;
    let source = fs::read_to_string(&args.board)
        .with_context(|| format!("reading {}", args.board.display()))?;
    let board = crate::lint::model::Board::read(&source)?;
    let text = match args.format {
        OutputFormat::Json => serde_json::to_string_pretty(&board)?,
        OutputFormat::Text => {
            let mut out = format!(
                "{}\n  tracks: {}\n  vias: {}\n  pads: {}\n  footprints: {}\n  zones: {}\n  copper layers: {} ({})\n  unmodeled geometry: {}",
                args.board.display(),
                board.tracks.len(),
                board.vias.len(),
                board.pads.len(),
                board.parts.len(),
                board.zones.len(),
                board.copper_layers.len(),
                board.copper_layers.join(", "),
                board.unmodeled_geometry.len(),
            );
            for (kind, n) in &board.unsupported {
                out.push_str(&format!("\n  unsupported {kind}: {n}"));
            }
            out
        }
    };
    emit(&text, args.output.as_deref(), &[&args.board])?;
    Ok(0)
}
fn corpus(args: CorpusArgs) -> Result<i32> {
    review::protect_output(args.output.as_deref(), &[&args.manifest])?;
    let manifest = crate::lint::corpus::Manifest::load(&args.manifest)?;
    let base = args.manifest.parent().unwrap_or(Path::new("."));
    let inputs = manifest.inputs(base);
    for input in &inputs {
        review::protect_output(args.output.as_deref(), &[input])?;
    }
    let results = manifest.evaluate(base)?;
    let failed = results.iter().filter(|r| !r.passed).count();
    eprintln!("{} corpus case(s), {failed} failed", results.len());
    emit(
        &serde_json::to_string_pretty(&results)?,
        args.output.as_deref(),
        &[&args.manifest],
    )?;
    Ok(if failed > 0 { 2 } else { 0 })
}
fn run_board(args: RunArgs) -> Result<i32> {
    protect_sheets(&args)?;
    let (source, policy, mut file, checked) = load(&args.board)?;
    if board_file::backfill(&mut file, &checked.report) > 0 {
        file.save(&policy)?;
    }
    warn_audit(&checked);
    if let Some(path) = &args.contact_sheet {
        fs::write(
            path,
            crate::lint::contact_sheet::render(&source, &checked.report)?,
        )?;
    }
    if let Some(path) = &args.exceptions_sheet {
        fs::write(
            path,
            crate::lint::contact_sheet::render_exceptions(
                &source,
                &checked.report,
                &file,
                &checked.audit,
            )?,
        )?;
    }
    match args.format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&RunOutput {
                board: &args.board.board,
                policy: &policy,
                report: &checked.report,
                exceptions: &checked.audit
            })?
        ),
        OutputFormat::Text => {
            let open = checked
                .report
                .findings
                .iter()
                .filter(|f| f.state != "ignored")
                .count();
            let ignored = checked
                .report
                .findings
                .iter()
                .filter(|f| f.state == "ignored")
                .count();
            println!(
                "{}: {open} open, {ignored} ignored, {} exceptions",
                args.board.board.display(),
                checked.audit.len()
            );
            for f in checked
                .report
                .findings
                .iter()
                .filter(|f| f.state != "ignored")
            {
                println!(
                    "{} {} {}: {}",
                    f.severity,
                    &f.key[..f.key.len().min(12)],
                    f.rule,
                    f.message
                );
            }
        }
    }
    Ok(if open_at(&checked.report, args.fail_on) > 0 {
        1
    } else {
        0
    })
}
fn decide(args: DecisionArgs, action: Action) -> Result<i32> {
    let (_source, policy, mut file, checked) = load(&args.board)?;
    board_file::upsert(
        &mut file,
        &checked.report,
        &args.key,
        action,
        &args.reason,
        &args.reviewer,
        args.expires_at.as_deref().map(parse_expiry).transpose()?,
    )?;
    file.save(&policy)?;
    println!("{} {} in {}", action, args.key, policy.display());
    Ok(0)
}

fn parse_expiry(value: &str) -> Result<u64> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Ok(seconds);
    }
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 3 {
        bail!("--expires-at must be Unix seconds or YYYY-MM-DD");
    }
    let year: i64 = parts[0].parse().context("invalid expiry year")?;
    let month: i64 = parts[1].parse().context("invalid expiry month")?;
    let day: i64 = parts[2].parse().context("invalid expiry day")?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month) || day < 1 || day > month_days[(month.saturating_sub(1)) as usize]
    {
        bail!("invalid expiry date {value}");
    }
    // Howard Hinnant's civil-date conversion: days since Unix epoch.
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days.checked_mul(86_400).context("expiry date overflow")?)
        .context("expiry must be after 1970-01-01")
}
fn prune(args: PruneArgs) -> Result<i32> {
    let (_source, policy, mut file, checked) = load(&args.board)?;
    warn_audit(&checked);
    let removed = board_file::prune(
        &mut file,
        &checked.audit,
        PruneSet {
            orphaned: !args.keep_orphaned,
            resolved: !args.keep_resolved,
            expired: !args.keep_expired,
            changed: args.changed,
        },
    );
    for e in &removed {
        println!("{} {} {}", e.action, e.key, e.reason);
    }
    if args.apply && !removed.is_empty() {
        file.save(&policy)?;
    }
    eprintln!(
        "{} {} exception(s){}",
        if args.apply {
            "removed"
        } else {
            "would remove"
        },
        removed.len(),
        if args.apply {
            ""
        } else {
            "; rerun with --apply"
        }
    );
    Ok(0)
}
fn diff(args: DiffArgs) -> Result<i32> {
    let mk = |board: &Path| -> Result<crate::lint::Report> {
        let source = fs::read_to_string(board)?;
        let policy =
            BoardLintFile::load_or_default(&BoardLintFile::path_for(board), &board_id(board))?;
        Ok(board_file::lint_board(&source, &policy, review::now())?.report)
    };
    let delta = board_file::diff(&mk(&args.before)?, &mk(&args.after)?);
    match args.format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&delta)?),
        OutputFormat::Text => println!(
            "{} new, {} fixed, {} unchanged",
            delta.new.len(),
            delta.fixed.len(),
            delta.unchanged
        ),
    }
    Ok(if delta.new.is_empty() { 0 } else { 1 })
}
fn ci(args: CiArgs) -> Result<i32> {
    fs::create_dir_all(&args.out)?;
    let mut failed = false;
    let mut index = String::from("# kct lint results\n\n");
    let mut retained = Vec::new();
    for board in args.boards {
        let ba = BoardArgs {
            board: board.clone(),
            file: None,
        };
        let (source, _policy, mut file, checked) = load(&ba)?;
        board_file::backfill(&mut file, &checked.report);
        warn_audit(&checked);
        let stem = artifact_key(&board);
        let dir = args.out.join(&stem);
        let summary = crate::lint::ci::write_bundle(&dir, &board, &source, &checked, &file)?;
        let open: usize = summary.open_by_severity.values().sum();
        let stale = summary.stale_exceptions.len();
        index.push_str(&format!("- **{stem}**: {open} open findings, {stale} stale exceptions ([findings](./{stem}/findings.html) · [exceptions](./{stem}/exceptions.html))\n"));
        let fail_on = match args.fail_on {
            FailOn::Never => crate::lint::ci::FailOn::Never,
            FailOn::Warning => crate::lint::ci::FailOn::Warning,
            FailOn::Error => crate::lint::ci::FailOn::Error,
        };
        failed |= !summary.passed(fail_on);
        retained.push((board, checked));
    }
    fs::write(args.out.join("summary.md"), index)?;
    let boards: Vec<_> = retained
        .iter()
        .map(|(path, checked)| (path.as_path(), checked))
        .collect();
    fs::write(
        args.out.join("findings.sarif"),
        serde_json::to_vec_pretty(&crate::lint::ci::sarif(&boards))?,
    )?;
    Ok(if failed { 1 } else { 0 })
}

fn artifact_key(board: &Path) -> String {
    let relative = std::env::current_dir()
        .ok()
        .and_then(|cwd| board.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| board.to_path_buf());
    let slug = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|component| {
            component
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("__");
    let digest = format!(
        "{:x}",
        Sha256::digest(relative.to_string_lossy().as_bytes())
    );
    format!("{slug}__{}", &digest[..8])
}

pub fn run(args: Vec<OsString>, _globals: &Globals) -> Result<i32> {
    match parse_args::<Cli>("lint", args).command {
        Command::Run(a) => run_board(a),
        Command::Init(a) => {
            if a.print_default_config {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&crate::lint::Config::default())?
                );
                return Ok(0);
            }
            let board = a.board.context("BOARD is required")?;
            fs::metadata(&board).with_context(|| format!("reading {}", board.display()))?;
            let path = BoardLintFile::path_for(&board);
            if path.exists() && !a.force {
                bail!("{} already exists (use --force)", path.display());
            }
            BoardLintFile::new(a.board_id.as_deref().unwrap_or(&board_id(&board))).save(&path)?;
            println!("created {}", path.display());
            Ok(0)
        }
        Command::Waive(a) => decide(a, Action::Ignore),
        Command::Flag(a) => decide(a, Action::Flag),
        Command::Clear(a) => {
            let path = a
                .board
                .file
                .clone()
                .unwrap_or_else(|| BoardLintFile::path_for(&a.board.board));
            let id = board_id(&a.board.board);
            let removed =
                BoardLintFile::update(&path, &id, |file| Ok(board_file::clear(file, &a.key)))?;
            if !removed {
                bail!("exception {} not found", a.key);
            }
            println!("cleared {} from {}", a.key, path.display());
            Ok(0)
        }
        Command::Stale(a) => {
            let (_, _, _, checked) = load(&a)?;
            for e in checked.stale() {
                println!("{} {} {}", e.status, e.key, e.detail);
            }
            Ok(if checked.stale().next().is_some() {
                1
            } else {
                0
            })
        }
        Command::Prune(a) => prune(a),
        Command::Diff(a) => diff(a),
        Command::Rules(a) => {
            let r = rules::catalog();
            if a.format == OutputFormat::Json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                for x in r {
                    println!("{:<36} {:<7} {}", x.id, x.severity, x.detection);
                }
            }
            Ok(0)
        }
        Command::Ci(a) => ci(a),
        Command::Inspect(a) => inspect(a),
        Command::Corpus(a) => corpus(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_accepts_dates_and_epoch_seconds() {
        assert_eq!(parse_expiry("1970-01-02").unwrap(), 86_400);
        assert_eq!(parse_expiry("2000000000").unwrap(), 2_000_000_000);
        assert!(parse_expiry("2025-02-29").is_err());
    }

    #[test]
    fn lifecycle_creates_policy_and_ci_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let board = temp.path().join("board.kicad_pcb");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/via_under_body.kicad_pcb"),
            &board,
        )
        .unwrap();
        assert_eq!(
            run(
                vec!["init".into(), board.as_os_str().to_owned()],
                &Globals::default()
            )
            .unwrap(),
            0
        );
        assert!(BoardLintFile::path_for(&board).exists());
        assert_eq!(
            run(
                vec![
                    "run".into(),
                    board.as_os_str().to_owned(),
                    "--fail-on".into(),
                    "never".into(),
                ],
                &Globals::default()
            )
            .unwrap(),
            0
        );
        let out = temp.path().join("ci");
        let _ = run(
            vec![
                "ci".into(),
                board.as_os_str().to_owned(),
                "--out".into(),
                out.as_os_str().to_owned(),
                "--fail-on".into(),
                "never".into(),
            ],
            &Globals::default(),
        )
        .unwrap();
        let bundle = fs::read_dir(&out)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .unwrap()
            .path();
        assert!(bundle.join("findings.html").exists());
        assert!(bundle.join("exceptions.html").exists());
        assert!(out.join("findings.sarif").exists());
    }
}
