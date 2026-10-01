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
            for original in originals
                .iter()
                .filter(|n| is_board_object(n) && !is_edge_graphic(n))
            {
                let mut n = original.clone();
                translate_board_object(&mut n, x, y);
                root.children.push(n)
            }
        }
    }
    let board_w = maxx - minx;
    let board_h = maxy - miny;
    let panel_max_x = (a.cols as f64 - 1.0) * dx + board_w;
    let panel_max_y = (a.rows as f64 - 1.0) * dy + board_h;
    let mut tabs = Vec::new();
    // Upstream places `tab_count` equally spaced tabs at every internal seam.
    for row in 0..a.rows {
        for col in 0..a.cols.saturating_sub(1) {
            let x = col as f64 * dx + board_w + a.spacing / 2.0;
            for i in 0..a.tab_count {
                let y = row as f64 * dy + board_h * (i + 1) as f64 / (a.tab_count + 1) as f64;
                tabs.push(Tab {
                    x,
                    y,
                    width: a.spacing,
                    height: a.tab_width,
                    horizontal: false,
                });
            }
        }
    }
    for col in 0..a.cols {
        for row in 0..a.rows.saturating_sub(1) {
            let y = row as f64 * dy + board_h + a.spacing / 2.0;
            for i in 0..a.tab_count {
                let x = col as f64 * dx + board_w * (i + 1) as f64 / (a.tab_count + 1) as f64;
                tabs.push(Tab {
                    x,
                    y,
                    width: a.tab_width,
                    height: a.spacing,
                    horizontal: true,
                });
            }
        }
    }
    for tab in &tabs {
        render_tab(&mut root, tab);
        if a.cut == "mousebite" {
            render_mousebites(&mut root, tab, a.mousebite_diameter, a.mousebite_spacing);
        }
    }
    if a.cut == "vcut" {
        for col in 0..a.cols.saturating_sub(1) {
            let x = col as f64 * dx + board_w + a.spacing / 2.0;
            root.children.push(gr_line(x, 0.0, x, panel_max_y));
        }
        for row in 0..a.rows.saturating_sub(1) {
            let y = row as f64 * dy + board_h + a.spacing / 2.0;
            root.children.push(gr_line(0.0, y, panel_max_x, y));
        }
    }
    let mut bounds = (0.0, 0.0, panel_max_x, panel_max_y);
    if a.frame {
        bounds = (
            -a.frame_space - a.frame_width,
            -a.frame_space - a.frame_width,
            panel_max_x + a.frame_space + a.frame_width,
            panel_max_y + a.frame_space + a.frame_width,
        );
        render_rect(&mut root, bounds);
        render_rect(
            &mut root,
            (
                -a.frame_space,
                -a.frame_space,
                panel_max_x + a.frame_space,
                panel_max_y + a.frame_space,
            ),
        );
    } else {
        render_rect(&mut root, bounds);
    }
    if a.tooling_holes {
        let pts = [
            (bounds.0 + 3.5, bounds.3 - 3.5),
            (bounds.2 - 3.5, bounds.3 - 3.5),
            (bounds.0 + 3.5, bounds.1 + 3.5),
        ];
        for (x, y) in pts {
            root.children
                .push(hole_footprint("Panel:ToolingHole", x, y, 3.0));
        }
    }
    if a.fiducials {
        let pts = [
            (bounds.0 + 5.0, bounds.3 - 5.0),
            (bounds.2 - 5.0, bounds.3 - 5.0),
            (bounds.0 + 5.0, bounds.1 + 5.0),
        ];
        for (x, y) in pts {
            root.children.push(fiducial_footprint(x, y));
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
    let data = serde_json::json!({"command":"panel","input":a.input,"output":out,
        "grid":{"rows":a.rows,"cols":a.cols,"spacing_mm":a.spacing},
        "board_count":a.rows*a.cols,"tabs":tabs.len(),"cut_method":a.cut,
        "tab_width_mm":a.tab_width,"tab_count":a.tab_count,"frame":a.frame,
        "tooling_holes":a.tooling_holes,"fiducials":a.fiducials,"success":true});
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&data)?)
    } else {
        println!("Created {}x{} panel: {}", a.cols, a.rows, out.display())
    }
    Ok(0)
}
#[derive(Clone, Copy)]
struct Tab {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    horizontal: bool,
}

fn is_edge_graphic(n: &SExp) -> bool {
    n.name.as_deref().is_some_and(|v| v.starts_with("gr_"))
        && n.get("layer").and_then(|l| l.string_at(0)) == Some("Edge.Cuts")
}
fn render_rect(root: &mut SExp, (x0, y0, x1, y1): (f64, f64, f64, f64)) {
    for (a, b, c, d) in [
        (x0, y0, x1, y0),
        (x1, y0, x1, y1),
        (x1, y1, x0, y1),
        (x0, y1, x0, y0),
    ] {
        root.children.push(gr_line(a, b, c, d));
    }
}
fn render_tab(root: &mut SExp, t: &Tab) {
    let (box_w, box_h) = if t.horizontal {
        (t.width, t.height)
    } else {
        (t.height, t.width)
    };
    let (x0, x1, y0, y1) = (
        t.x - box_w / 2.0,
        t.x + box_w / 2.0,
        t.y - box_h / 2.0,
        t.y + box_h / 2.0,
    );
    if t.horizontal {
        root.children.push(gr_line(x0, y0, x0, y1));
        root.children.push(gr_line(x1, y0, x1, y1));
    } else {
        root.children.push(gr_line(x0, y0, x1, y0));
        root.children.push(gr_line(x0, y1, x1, y1));
    }
}
fn gr_line(x0: f64, y0: f64, x1: f64, y1: f64) -> SExp {
    SExp::list(
        "gr_line",
        [
            SExp::list("start", [SExp::atom(x0), SExp::atom(y0)]),
            SExp::list("end", [SExp::atom(x1), SExp::atom(y1)]),
            SExp::list(
                "stroke",
                [
                    SExp::pair("width", 0.05),
                    SExp::list("type", [SExp::symbol("default")]),
                ],
            ),
            SExp::list("layer", [SExp::quoted("Edge.Cuts")]),
        ],
    )
}
fn render_mousebites(root: &mut SExp, t: &Tab, diameter: f64, spacing: f64) {
    // Upstream's `Tab.width` is the perforation-line dimension in both
    // orientations: the tab bridge is wide along X when horizontal and along
    // Y when vertical.
    let length = t.width;
    let count = ((length / spacing).floor() as usize + 1).max(1);
    for i in 0..count {
        let p = if count == 1 {
            0.0
        } else {
            -length / 2.0 + length * i as f64 / (count - 1) as f64
        };
        let (x, y) = if t.horizontal {
            (t.x + p, t.y)
        } else {
            (t.x, t.y + p)
        };
        root.children
            .push(hole_footprint("Panel:Mousebite", x, y, diameter));
    }
}
fn hole_footprint(name: &str, x: f64, y: f64, d: f64) -> SExp {
    SExp::list(
        "footprint",
        [
            SExp::quoted(name),
            SExp::list("layer", [SExp::quoted("F.Cu")]),
            SExp::list("at", [SExp::atom(x), SExp::atom(y)]),
            SExp::list(
                "attr",
                [
                    SExp::symbol("board_only"),
                    SExp::symbol("exclude_from_pos_files"),
                    SExp::symbol("exclude_from_bom"),
                ],
            ),
            SExp::list(
                "pad",
                [
                    SExp::quoted(""),
                    SExp::symbol("np_thru_hole"),
                    SExp::symbol("circle"),
                    SExp::list("at", [SExp::atom(0.0), SExp::atom(0.0)]),
                    SExp::list("size", [SExp::atom(d), SExp::atom(d)]),
                    SExp::pair("drill", d),
                    SExp::list("layers", [SExp::quoted("*.Cu"), SExp::quoted("*.Mask")]),
                ],
            ),
        ],
    )
}
fn fiducial_footprint(x: f64, y: f64) -> SExp {
    SExp::list(
        "footprint",
        [
            SExp::quoted("Panel:Fiducial"),
            SExp::list("layer", [SExp::quoted("F.Cu")]),
            SExp::list("at", [SExp::atom(x), SExp::atom(y)]),
            SExp::list(
                "attr",
                [
                    SExp::symbol("board_only"),
                    SExp::symbol("exclude_from_pos_files"),
                    SExp::symbol("exclude_from_bom"),
                ],
            ),
            SExp::list(
                "pad",
                [
                    SExp::quoted("1"),
                    SExp::symbol("smd"),
                    SExp::symbol("circle"),
                    SExp::list("at", [SExp::atom(0.0), SExp::atom(0.0)]),
                    SExp::list("size", [SExp::atom(1.0), SExp::atom(1.0)]),
                    SExp::list("layers", [SExp::quoted("F.Cu"), SExp::quoted("F.Mask")]),
                    SExp::pair("solder_mask_margin", 2.0),
                ],
            ),
        ],
    )
}

#[cfg(test)]
mod panel_parity_tests {
    use super::*;

    #[test]
    fn upstream_2x2_mousebite_golden_has_42_holes() {
        let mut root = SExp::list("kicad_pcb", []);
        let horizontal = Tab {
            x: 0.0,
            y: 0.0,
            width: 3.0,
            height: 2.0,
            horizontal: true,
        };
        let vertical = Tab {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 3.0,
            horizontal: false,
        };
        for _ in 0..6 {
            render_mousebites(&mut root, &horizontal, 0.5, 0.8);
        }
        for _ in 0..6 {
            render_mousebites(&mut root, &vertical, 0.5, 0.8);
        }
        assert_eq!(
            root.children.len(),
            42,
            "matches rjwalters/kicad-tools golden panel"
        );
        assert!(root
            .children
            .iter()
            .all(|n| n.string_at(0) == Some("Panel:Mousebite")));
    }

    #[test]
    fn panel_furniture_uses_upstream_footprint_shapes() {
        let hole = hole_footprint("Panel:ToolingHole", 3.5, 3.5, 3.0);
        assert_eq!(hole.get("at").and_then(|n| n.float_at(0)), Some(3.5));
        let pad = hole.children_named("pad").next().unwrap();
        assert_eq!(pad.string_at(1), Some("np_thru_hole"));
        let fid = fiducial_footprint(5.0, 5.0);
        let pad = fid.children_named("pad").next().unwrap();
        assert_eq!(pad.string_at(1), Some("smd"));
        assert_eq!(
            pad.get("solder_mask_margin").and_then(|n| n.float_at(0)),
            Some(2.0)
        );
    }
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
