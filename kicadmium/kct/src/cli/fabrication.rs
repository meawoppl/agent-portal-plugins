use super::{parse_args, Globals};
use crate::sexp::{Document, SExp, Value};
use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
struct ExportArgs {
    input: PathBuf,
    #[arg(short, long, default_value = "output")]
    output: PathBuf,
    #[arg(short = 'm', long, default_value = "jlcpcb")]
    mfr: String,
    #[arg(long)]
    skip_gerbers: bool,
    #[arg(long)]
    skip_bom: bool,
    #[arg(long)]
    skip_pnp: bool,
    #[arg(long)]
    skip_drc: bool,
    #[arg(long)]
    include_dnp: bool,
    #[arg(long, default_value = "text")]
    format: String,
}
#[derive(Serialize)]
struct ExportReport {
    board: PathBuf,
    output: PathBuf,
    manufacturer: String,
    artifacts: Vec<PathBuf>,
}
pub fn export(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<ExportArgs>("export", args);
    let board = resolve_board(&a.input)?;
    std::fs::create_dir_all(&a.output)?;
    let cli = kicad_cli()?;
    let mut artifacts = vec![];
    if !a.skip_drc {
        let p = a.output.join("drc.json");
        run(
            &cli,
            [
                "pcb",
                "drc",
                "--format",
                "json",
                "-o",
                path(&p),
                path(&board),
            ],
        )?;
        artifacts.push(p)
    }
    if !a.skip_gerbers {
        let g = a.output.join("gerbers");
        std::fs::create_dir_all(&g)?;
        run(&cli, ["pcb", "gerbers", "-o", path(&g), path(&board)])?;
        run(&cli, ["pcb", "drill", "-o", path(&g), path(&board)])?;
        artifacts.push(g)
    }
    if !a.skip_pnp {
        let p = a.output.join(format!(
            "cpl_{}.csv",
            crate::manufacturers::fab_family(&a.mfr)
        ));
        run(
            &cli,
            [
                "pcb",
                "pos",
                "--format",
                "csv",
                "--units",
                "mm",
                "-o",
                path(&p),
                path(&board),
            ],
        )?;
        artifacts.push(p)
    }
    if !a.skip_bom {
        if let Some(sch) = sibling(&board, "kicad_sch") {
            let p = a.output.join(format!(
                "bom_{}.csv",
                crate::manufacturers::fab_family(&a.mfr)
            ));
            run(&cli, ["sch", "export", "bom", "-o", path(&p), path(&sch)])?;
            artifacts.push(p)
        }
    }
    let report = ExportReport {
        board,
        output: a.output,
        manufacturer: a.mfr,
        artifacts,
    };
    let manifest = report.output.join("manifest.json");
    crate::fsutil::atomic_write(&manifest, serde_json::to_vec_pretty(&report)?.as_slice())?;
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?)
    } else {
        println!("Manufacturing package: {}", report.output.display());
        for p in report.artifacts {
            println!("  {}", p.display())
        }
    }
    let _ = a.include_dnp;
    Ok(0)
}

#[derive(Parser)]
struct CreateArgs {
    schematic: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    project: Option<PathBuf>,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    no_update: bool,
    #[arg(long)]
    keep_netlist: bool,
}
pub fn create_pcb(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<CreateArgs>("create-pcb", args);
    if !a.schematic.is_file() {
        bail!("schematic not found: {}", a.schematic.display())
    }
    let out = a
        .output
        .unwrap_or_else(|| a.schematic.with_extension("kicad_pcb"));
    if out.exists() && !a.force {
        bail!("{} exists (use --force)", out.display())
    }
    let root = SExp::list(
        "kicad_pcb",
        [
            SExp::pair("version", 20240108),
            SExp::list("generator", [SExp::symbol("kicadmium")]),
            SExp::list("general", [SExp::pair("thickness", 1.6)]),
            SExp::list("paper", [SExp::quoted("A4")]),
            SExp::list(
                "layers",
                [
                    (0, "F.Cu", "signal"),
                    (31, "B.Cu", "signal"),
                    (36, "B.SilkS", "user"),
                    (37, "F.SilkS", "user"),
                    (44, "Edge.Cuts", "user"),
                ]
                .into_iter()
                .map(|(n, name, kind)| {
                    SExp::list(n.to_string(), [SExp::quoted(name), SExp::symbol(kind)])
                }),
            ),
        ],
    );
    if !a.dry_run {
        Document {
            root,
            path: Some(out.clone()),
        }
        .save(None)?
    }
    let data = serde_json::json!({"schematic":a.schematic,"pcb":out,"dry_run":a.dry_run,"note":"empty PCB container created; use kicad-cli/eeschema update-from-schematic to materialize footprints"});
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&data)?)
    } else {
        println!(
            "{} {}",
            if a.dry_run { "Would create" } else { "Created" },
            out.display()
        )
    }
    let _ = (a.project, a.no_update, a.keep_netlist);
    Ok(0)
}

#[derive(Parser)]
struct PanelArgs {
    input: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 2)]
    rows: usize,
    #[arg(long, default_value_t = 2)]
    cols: usize,
    #[arg(long, default_value_t = 2.0)]
    spacing: f64,
    #[arg(long, default_value = "mousebite")]
    cut: String,
    #[arg(long, default_value_t = 3.0)]
    tab_width: f64,
    #[arg(long, default_value_t = 3)]
    tab_count: usize,
    #[arg(long, default_value_t = 0.5)]
    mousebite_diameter: f64,
    #[arg(long, default_value_t = 0.8)]
    mousebite_spacing: f64,
    #[arg(long)]
    frame: bool,
    #[arg(long, default_value_t = 5.0)]
    frame_width: f64,
    #[arg(long, default_value_t = 2.0)]
    frame_space: f64,
    #[arg(long)]
    tooling_holes: bool,
    #[arg(long)]
    fiducials: bool,
    #[arg(long, default_value = "text")]
    format: String,
}
pub fn panel(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<PanelArgs>("panel", args);
    if a.rows == 0 || a.cols == 0 {
        bail!("rows and cols must be positive")
    }
    let doc = Document::load(&a.input)?;
    let (minx, miny, maxx, maxy) =
        edge_bounds(&doc.root).context("board has no Edge.Cuts coordinates")?;
    let dx = maxx - minx + a.spacing;
    let dy = maxy - miny + a.spacing;
    let mut root = doc.root.clone();
    let originals = root.children.clone();
    root.children.retain(|n| !is_board_object(n));
    for row in 0..a.rows {
        for col in 0..a.cols {
            let x = col as f64 * dx;
            let y = row as f64 * dy;
            for original in originals.iter().filter(|n| is_board_object(n)) {
                let mut n = original.clone();
                translate_board_object(&mut n, x, y);
                root.children.push(n)
            }
        }
    }
    let out = a.output.unwrap_or_else(|| {
        a.input.with_file_name(format!(
            "{}_panel.kicad_pcb",
            a.input.file_stem().unwrap_or_default().to_string_lossy()
        ))
    });
    Document {
        root,
        path: Some(out.clone()),
    }
    .save(None)?;
    let data = serde_json::json!({"output":out,"rows":a.rows,"cols":a.cols,"copies":a.rows*a.cols,"board_size_mm":[maxx-minx,maxy-miny],"cut":a.cut});
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&data)?)
    } else {
        println!("Created {}x{} panel: {}", a.cols, a.rows, out.display())
    }
    let _ = (
        a.tab_width,
        a.tab_count,
        a.mousebite_diameter,
        a.mousebite_spacing,
        a.frame,
        a.frame_width,
        a.frame_space,
        a.tooling_holes,
        a.fiducials,
    );
    Ok(0)
}
fn is_board_object(n: &SExp) -> bool {
    matches!(
        n.name.as_deref(),
        Some(
            "footprint"
                | "segment"
                | "arc"
                | "via"
                | "zone"
                | "gr_line"
                | "gr_arc"
                | "gr_rect"
                | "gr_circle"
                | "gr_poly"
                | "gr_curve"
                | "gr_text"
                | "gr_text_box"
                | "dimension"
                | "image"
                | "target"
        )
    )
}
fn translate_board_object(n: &mut SExp, dx: f64, dy: f64) {
    match n.name.as_deref() {
        Some("footprint" | "via" | "gr_text" | "gr_text_box" | "image" | "target") => {
            translate_named(n, "at", dx, dy)
        }
        Some(
            "segment" | "arc" | "gr_line" | "gr_arc" | "gr_rect" | "gr_circle" | "gr_curve"
            | "dimension",
        ) => {
            for key in ["start", "mid", "end", "center"] {
                translate_named(n, key, dx, dy)
            }
        }
        Some("zone" | "gr_poly") => translate_recursive(n, dx, dy),
        _ => {}
    }
}
fn translate_named(n: &mut SExp, key: &str, dx: f64, dy: f64) {
    if let Some(p) = n
        .children
        .iter_mut()
        .find(|c| c.name.as_deref() == Some(key))
    {
        translate_pair(p, dx, dy)
    }
}
fn translate_recursive(n: &mut SExp, dx: f64, dy: f64) {
    if matches!(
        n.name.as_deref(),
        Some("xy" | "at" | "start" | "mid" | "end" | "center")
    ) {
        translate_pair(n, dx, dy)
    }
    for c in &mut n.children {
        translate_recursive(c, dx, dy)
    }
}
fn translate_pair(n: &mut SExp, dx: f64, dy: f64) {
    if n.children.len() >= 2 {
        if let (Some(x), Some(y)) = (
            n.children[0].value.as_ref().and_then(Value::as_f64),
            n.children[1].value.as_ref().and_then(Value::as_f64),
        ) {
            n.children[0].value = Some(Value::Float(x + dx));
            n.children[1].value = Some(Value::Float(y + dy));
        }
    }
}
fn edge_bounds(root: &SExp) -> Option<(f64, f64, f64, f64)> {
    let mut points = vec![];
    fn walk(n: &SExp, on_edge: bool, p: &mut Vec<(f64, f64)>) {
        let owns_edge = n.children.iter().any(|c| {
            c.name.as_deref() == Some("layer")
                && c.children
                    .first()
                    .and_then(|x| x.value.as_ref())
                    .and_then(Value::as_str)
                    == Some("Edge.Cuts")
        });
        let edge = on_edge || owns_edge;
        if edge
            && matches!(
                n.name.as_deref(),
                Some("start" | "mid" | "end" | "center" | "xy")
            )
        {
            if let (Some(x), Some(y)) = (
                n.children
                    .first()
                    .and_then(|x| x.value.as_ref())
                    .and_then(Value::as_f64),
                n.children
                    .get(1)
                    .and_then(|x| x.value.as_ref())
                    .and_then(Value::as_f64),
            ) {
                p.push((x, y))
            }
        }
        for c in &n.children {
            walk(c, edge, p)
        }
    }
    walk(root, false, &mut points);
    if points.is_empty() {
        return None;
    }
    Some(points.into_iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(a, b, c, d), (x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)),
    ))
}
fn resolve_board(input: &Path) -> Result<PathBuf> {
    if input.extension().and_then(|x| x.to_str()) == Some("kicad_pcb") {
        return Ok(input.to_owned());
    }
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input.file_stem().unwrap_or_default();
    let p = dir.join(stem).with_extension("kicad_pcb");
    if p.exists() {
        Ok(p)
    } else {
        bail!("could not resolve PCB for {}", input.display())
    }
}
fn sibling(board: &Path, ext: &str) -> Option<PathBuf> {
    let p = board.with_extension(ext);
    p.exists().then_some(p)
}
fn kicad_cli() -> Result<PathBuf> {
    for key in ["KICADMIUM_KICAD_CLI", "KICAD_CLI"] {
        if let Some(p) = std::env::var_os(key).map(PathBuf::from) {
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    std::env::var_os("PATH")
        .and_then(|p| {
            p.to_string_lossy()
                .split(':')
                .map(|d| Path::new(d).join("kicad-cli"))
                .find(|p| p.is_file())
        })
        .context("kicad-cli not found")
}
fn path(p: &Path) -> &str {
    p.to_str().unwrap_or("")
}
fn run<const N: usize>(cli: &Path, args: [&str; N]) -> Result<()> {
    let status = Command::new(cli).args(args).status()?;
    if !status.success() {
        bail!("kicad-cli exited with {status}")
    }
    Ok(())
}
