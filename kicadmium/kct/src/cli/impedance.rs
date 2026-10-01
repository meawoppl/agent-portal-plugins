//! `kct impedance`: transmission line calculations (port of
//! `kicad_tools.cli.commands.impedance`).
//!
//! Subcommands: `stackup`, `width`, `calculate`, `diffpair`, `crosstalk`.
//! Text output reproduces upstream's Rich rendering on a non-terminal
//! (markup stripped, 80-column word wrap, heavy-head table); JSON output
//! matches `json.dumps(..., indent=2)` including key order and float repr.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

use super::{parse_args, Globals};
use crate::physics::stackup::{Construction, StackupSummary};
use crate::physics::{
    py_float_repr, py_round, CoupledLines, CrosstalkAnalyzer, ImpedanceResult, PhysResult, Stackup,
    TransmissionLine,
};

#[derive(Parser)]
#[command(
    about = "Transmission line impedance calculations",
    long_about = "Calculate trace impedance, width for target impedance, differential pair \
                  parameters, and crosstalk estimation"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Preset {
    #[value(name = "jlcpcb-4")]
    Jlcpcb4,
    #[value(name = "jlcpcb-4-legacy")]
    Jlcpcb4Legacy,
    #[value(name = "jlcpcb-3313")]
    Jlcpcb3313,
    #[value(name = "jlcpcb-7628")]
    Jlcpcb7628,
    #[value(name = "oshpark-4")]
    Oshpark4,
    #[value(name = "generic-2")]
    Generic2,
    #[value(name = "generic-4")]
    Generic4,
    #[value(name = "generic-6")]
    Generic6,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum WidthMode {
    Auto,
    Microstrip,
    Stripline,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CalcMode {
    Auto,
    Microstrip,
    Stripline,
    Cpwg,
}

#[derive(ClapArgs, Clone)]
struct Common {
    /// Path to .kicad_pcb file (optional if --preset used)
    pcb: Option<PathBuf>,
    /// Use a preset stackup instead of reading from PCB
    #[arg(short = 'p', long = "preset", value_enum)]
    preset: Option<Preset>,
    /// Output format (default: text)
    #[arg(short = 'f', long = "format", value_enum, default_value = "text")]
    format: Format,
}

#[derive(Subcommand)]
enum Command {
    /// Display parsed board stackup
    #[command(long_about = "Show layer stackup extracted from PCB or from preset")]
    Stackup {
        #[command(flatten)]
        common: Common,
    },
    /// Calculate trace width for target impedance
    #[command(long_about = "Find trace width needed to achieve a target characteristic impedance")]
    Width {
        #[command(flatten)]
        common: Common,
        /// Target characteristic impedance in ohms (e.g., 50)
        #[arg(short = 'z', long = "target")]
        target: f64,
        /// Layer name (e.g., F.Cu, In1.Cu)
        #[arg(short = 'l', long = "layer")]
        layer: String,
        /// Transmission line mode (default: auto-detect from layer position)
        #[arg(short = 'm', long = "mode", value_enum, default_value = "auto")]
        mode: WidthMode,
    },
    /// Calculate impedance for given geometry
    #[command(long_about = "Forward calculation: given trace width, compute impedance")]
    Calculate {
        #[command(flatten)]
        common: Common,
        /// Trace width in mm
        #[arg(short = 'w', long = "width", allow_negative_numbers = true)]
        width: f64,
        /// Layer name (e.g., F.Cu, In1.Cu)
        #[arg(short = 'l', long = "layer")]
        layer: String,
        /// Transmission line mode (default: auto-detect)
        #[arg(short = 'm', long = "mode", value_enum, default_value = "auto")]
        mode: CalcMode,
        /// Gap to ground for CPWG mode in mm
        #[arg(short = 'g', long = "gap", allow_negative_numbers = true)]
        gap: Option<f64>,
        /// Signal frequency in GHz for loss calculation (default: 1.0)
        #[arg(long = "frequency", default_value_t = 1.0)]
        frequency: f64,
    },
    /// Analyze differential pair
    #[command(long_about = "Calculate differential and common mode impedances for coupled lines")]
    Diffpair {
        #[command(flatten)]
        common: Common,
        /// Trace width in mm (each trace)
        #[arg(short = 'w', long = "width", allow_negative_numbers = true)]
        width: f64,
        /// Edge-to-edge gap between traces in mm
        #[arg(short = 'g', long = "gap", allow_negative_numbers = true)]
        gap: f64,
        /// Layer name (e.g., F.Cu, In1.Cu)
        #[arg(short = 'l', long = "layer")]
        layer: String,
        /// Target differential impedance (e.g., 90, 100) for verification
        #[arg(short = 'z', long = "target")]
        target: Option<f64>,
    },
    /// Estimate crosstalk between parallel traces
    #[command(long_about = "Calculate NEXT/FEXT crosstalk, or find spacing for budget")]
    Crosstalk {
        #[command(flatten)]
        common: Common,
        /// Layer name (e.g., F.Cu, In1.Cu)
        #[arg(short = 'l', long = "layer")]
        layer: String,
        /// Trace width in mm (both traces)
        #[arg(short = 'w', long = "width", allow_negative_numbers = true)]
        width: Option<f64>,
        /// Edge-to-edge spacing in mm (for analysis mode)
        #[arg(short = 's', long = "spacing", allow_negative_numbers = true)]
        spacing: Option<f64>,
        /// Parallel run length in mm
        #[arg(long = "length", allow_negative_numbers = true)]
        length: Option<f64>,
        /// Signal rise time in nanoseconds (default: 1.0)
        #[arg(long = "rise-time", default_value_t = 1.0)]
        rise_time: f64,
        /// Maximum crosstalk % (for spacing calculation mode)
        #[arg(long = "max-percent")]
        max_percent: Option<f64>,
    },
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let out = execute(args);
    print!("{}", out.stdout);
    eprint!("{}", out.stderr);
    Ok(out.code)
}

/// Captured result of one `kct impedance` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run `kct impedance <args>` and capture its output instead of printing.
/// Argument errors (and `--help`) still exit the process via clap.
pub fn execute(args: Vec<OsString>) -> Output {
    let args: Args = parse_args("impedance", args);
    let mut out = Console::default();
    let Some(command) = args.command else {
        out.raw("Usage: kct impedance <command> [options] [board.kicad_pcb]\n");
        out.raw("Commands: stackup, width, calculate, diffpair, crosstalk\n");
        return Output {
            code: 1,
            stdout: out.take(),
            stderr: String::new(),
        };
    };
    let (code, stderr) = match dispatch(command, &mut out) {
        Ok(code) | Err(Failure::Exit(code)) => (code, String::new()),
        Err(Failure::Value(msg)) => (1, format!("Error: ValueError: {msg}\n")),
        Err(Failure::Other(msg)) => (1, format!("{msg}\n")),
    };
    Output {
        code,
        stdout: out.take(),
        stderr,
    }
}

enum Failure {
    /// Upstream `sys.exit(code)` after printing to stdout.
    Exit(i32),
    /// Uncaught `ValueError` reported by the top-level handler.
    Value(String),
    /// Other uncaught errors (already formatted).
    Other(String),
}

impl From<crate::physics::ValueError> for Failure {
    fn from(e: crate::physics::ValueError) -> Self {
        Failure::Value(e.0)
    }
}

type CmdResult = std::result::Result<i32, Failure>;

fn dispatch(command: Command, out: &mut Console) -> CmdResult {
    match command {
        Command::Stackup { common } => run_stackup(&common, out),
        Command::Width {
            common,
            target,
            layer,
            mode,
        } => run_width(&common, target, &layer, mode, out),
        Command::Calculate {
            common,
            width,
            layer,
            mode,
            gap,
            frequency,
        } => run_calculate(&common, width, &layer, mode, gap, frequency, out),
        Command::Diffpair {
            common,
            width,
            gap,
            layer,
            target,
        } => run_diffpair(&common, width, gap, &layer, target, out),
        Command::Crosstalk {
            common,
            layer,
            width,
            spacing,
            length,
            rise_time,
            max_percent,
        } => run_crosstalk(
            &common,
            &layer,
            width,
            spacing,
            length,
            rise_time,
            max_percent,
            out,
        ),
    }
}

fn get_stackup(common: &Common, out: &mut Console) -> std::result::Result<Stackup, Failure> {
    if let Some(path) = &common.pcb {
        if !path.exists() {
            return Err(Failure::Other(format!(
                "Error: PCB file not found\n\nContext:\n  file: {}\n\nSuggestions:\n  \
                 - Check that the file path is correct\n  \
                 - Ensure the file has a .kicad_pcb extension",
                path.display()
            )));
        }
        return Stackup::load(path).map_err(|e| Failure::Other(format!("Error: {e:#}")));
    }
    let Some(preset) = common.preset else {
        out.raw("Error: Provide either BOARD path or --preset option\n");
        return Err(Failure::Exit(1));
    };
    Ok(match preset {
        Preset::Jlcpcb4 | Preset::Generic4 => Stackup::jlcpcb_4layer(),
        Preset::Jlcpcb4Legacy => Stackup::jlcpcb_4layer_legacy(),
        Preset::Jlcpcb3313 => Stackup::jlcpcb_named("JLC04161H-3313")?,
        Preset::Jlcpcb7628 => Stackup::jlcpcb_named("JLC04161H-7628")?,
        Preset::Oshpark4 => Stackup::oshpark_4layer(),
        Preset::Generic2 => Stackup::default_2layer(1.6),
        Preset::Generic6 => Stackup::default_6layer(),
    })
}

fn f(x: f64) -> String {
    py_float_repr(x)
}

// ------------------------------------------------------------------ stackup

fn run_stackup(common: &Common, out: &mut Console) -> CmdResult {
    let stackup = get_stackup(common, out)?;
    if common.format == Format::Json {
        out.raw(&(summary_json(&stackup.summary()).dumps() + "\n"));
        return Ok(0);
    }
    if let Some(c) = &stackup.construction {
        out.print(&format!("Construction: {}", c.id()));
        if c.compatibility_only() {
            out.print("Legacy numerical model; no factory ordering identifier.");
        }
    }
    out.print(&format!(
        "\nBoard Stackup ({:.2}mm total):\n",
        stackup.board_thickness_mm
    ));
    let rows: Vec<Vec<String>> = stackup
        .layers
        .iter()
        .map(|layer| {
            let thickness = if layer.thickness_mm < 0.1 {
                let mut t = format!("{:.1}μm", layer.thickness_mm * 1000.0);
                if let Some(oz) = layer.copper_weight_oz.filter(|oz| *oz != 0.0) {
                    t += &format!(" ({oz:.1}oz)");
                }
                t
            } else {
                format!("{:.3}mm", layer.thickness_mm)
            };
            let er = if layer.epsilon_r > 0.0 {
                format!("{:.2}", layer.epsilon_r)
            } else {
                "-".into()
            };
            vec![
                layer.name.clone(),
                capitalize(layer.layer_type.value()),
                thickness,
                er,
                if layer.material.is_empty() {
                    "-".into()
                } else {
                    layer.material.clone()
                },
            ]
        })
        .collect();
    out.raw(&render_table(
        &[
            ("Layer", false),
            ("Type", false),
            ("Thickness", true),
            ("εr", true),
            ("Material", false),
        ],
        &rows,
    ));
    if !stackup.copper_finish.is_empty() {
        out.print(&format!("\nCopper finish: {}", stackup.copper_finish));
    }
    Ok(0)
}

/// Python `str.capitalize()`.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c
            .to_uppercase()
            .chain(chars.flat_map(|c| c.to_lowercase()))
            .collect(),
        None => String::new(),
    }
}

// -------------------------------------------------------------------- width

fn run_width(
    common: &Common,
    target: f64,
    layer: &str,
    mode: WidthMode,
    out: &mut Console,
) -> CmdResult {
    let stackup = get_stackup(common, out)?;
    let tl = TransmissionLine::new(stackup.clone());
    let mode_str = match mode {
        WidthMode::Auto => "auto",
        WidthMode::Microstrip => "microstrip",
        WidthMode::Stripline => "stripline",
    };
    let width_mm = match tl.width_for_impedance(target, layer, mode_str) {
        Ok(w) => w,
        Err(e) => {
            out.raw(&format!("Error: {e}\n"));
            return Ok(1);
        }
    };
    let is_outer = stackup.is_outer_layer(layer);
    let microstrip = match mode {
        WidthMode::Auto => is_outer,
        WidthMode::Microstrip => true,
        WidthMode::Stripline => false,
    };
    let geometry = if microstrip {
        "microstrip"
    } else {
        "stripline"
    };
    let result = if microstrip {
        tl.microstrip(width_mm, layer, 1.0)?
    } else {
        tl.stripline(width_mm, layer, 1.0)?
    };

    if common.format == Format::Json {
        let data = Json::obj(vec![
            ("target_impedance_ohm", Json::Num(target)),
            ("layer", Json::str(layer)),
            ("geometry", Json::str(geometry)),
            ("width_mm", Json::Num(py_round(width_mm, 4))),
            ("width_mil", Json::Num(py_round(width_mm / 0.0254, 2))),
            (
                "verification",
                Json::obj(vec![
                    ("z0_ohm", Json::Num(py_round(result.z0, 2))),
                    ("epsilon_eff", Json::Num(py_round(result.epsilon_eff, 3))),
                    (
                        "delay_ps_per_mm",
                        Json::Num(py_round(result.propagation_delay_ps_per_mm(), 2)),
                    ),
                ]),
            ),
            (
                "construction",
                construction_json(stackup.construction.as_ref()),
            ),
        ]);
        out.raw(&(data.dumps() + "\n"));
        return Ok(0);
    }

    out.print(&format!(
        "\nTrace Width for {}Ω on {layer} ({geometry}):\n",
        f(target)
    ));
    let h = stackup.get_reference_plane_distance(layer)?;
    let er = stackup.get_dielectric_constant(layer);
    out.print(&format!("  Target:     {}Ω", f(target)));
    out.print(&format!(
        "  Layer:      {layer} ({} layer, {geometry})",
        if is_outer { "outer" } else { "inner" }
    ));
    out.print(&format!("  Stackup:    {h:.3}mm to reference, εr={er:.2}"));
    out.print("");
    out.print(&format!(
        "  Result:     {:.1} mil ({width_mm:.3} mm)",
        width_mm * 1000.0 / 25.4
    ));
    out.print("");
    out.print("  Verification:");
    out.print(&format!("    Z₀ = {:.1}Ω", result.z0));
    out.print(&format!("    εeff = {:.2}", result.epsilon_eff));
    out.print(&format!(
        "    Delay = {:.1} ps/mm",
        result.propagation_delay_ps_per_mm()
    ));
    Ok(0)
}

// ---------------------------------------------------------------- calculate

#[allow(clippy::too_many_arguments)]
fn run_calculate(
    common: &Common,
    width_mm: f64,
    layer: &str,
    mode: CalcMode,
    gap_mm: Option<f64>,
    freq_ghz: f64,
    out: &mut Console,
) -> CmdResult {
    let stackup = get_stackup(common, out)?;
    let tl = TransmissionLine::new(stackup.clone());
    let is_outer = stackup.is_outer_layer(layer);
    let micro = |tl: &TransmissionLine| -> PhysResult<(ImpedanceResult, &'static str)> {
        Ok((tl.microstrip(width_mm, layer, freq_ghz)?, "Microstrip"))
    };
    let strip = |tl: &TransmissionLine| -> PhysResult<(ImpedanceResult, &'static str)> {
        Ok((tl.stripline(width_mm, layer, freq_ghz)?, "Stripline"))
    };
    let (result, geometry) = match (mode, gap_mm) {
        (CalcMode::Cpwg, Some(gap)) => (tl.cpwg(width_mm, gap, layer, freq_ghz)?, "CPWG"),
        (CalcMode::Microstrip, _) => micro(&tl)?,
        (CalcMode::Stripline, _) => strip(&tl)?,
        _ if is_outer => micro(&tl)?,
        _ => strip(&tl)?,
    };
    let h = stackup.get_reference_plane_distance(layer)?;
    let er = stackup.get_dielectric_constant(layer);
    let gap_truthy = gap_mm.filter(|g| *g != 0.0);

    if common.format == Format::Json {
        let mut fields = vec![
            ("geometry", Json::str(&geometry.to_lowercase())),
            ("layer", Json::str(layer)),
            ("width_mm", Json::Num(width_mm)),
            ("width_mil", Json::Num(py_round(width_mm / 0.0254, 2))),
            ("height_mm", Json::Num(py_round(h, 4))),
            ("epsilon_r", Json::Num(py_round(er, 2))),
            (
                "results",
                Json::obj(vec![
                    ("z0_ohm", Json::Num(py_round(result.z0, 2))),
                    ("epsilon_eff", Json::Num(py_round(result.epsilon_eff, 3))),
                    (
                        "delay_ps_per_mm",
                        Json::Num(py_round(result.propagation_delay_ps_per_mm(), 2)),
                    ),
                    (
                        "delay_ps_per_inch",
                        Json::Num(py_round(result.propagation_delay_ns_per_inch() * 1000.0, 1)),
                    ),
                    (
                        "loss_db_per_m",
                        Json::Num(py_round(result.loss_db_per_m, 3)),
                    ),
                    (
                        "loss_db_per_inch",
                        Json::Num(py_round(result.loss_db_per_m * 0.0254, 4)),
                    ),
                ]),
            ),
        ];
        if let Some(gap) = gap_truthy {
            fields.push(("gap_mm", Json::Num(gap)));
        }
        fields.push((
            "construction",
            construction_json(stackup.construction.as_ref()),
        ));
        out.raw(&(Json::obj(fields).dumps() + "\n"));
        return Ok(0);
    }

    out.print("\nImpedance Calculation:\n");
    out.print(&format!("  Geometry:   {geometry}"));
    out.print(&format!(
        "  Width:      {:.1} mil ({width_mm:.3} mm)",
        width_mm / 0.0254
    ));
    out.print(&format!("  Layer:      {layer}"));
    out.print(&format!(
        "  Height:     {:.1} mil ({h:.3} mm) to reference",
        h / 0.0254
    ));
    out.print(&format!("  εr:         {er:.2}"));
    if let Some(gap) = gap_truthy {
        out.print(&format!(
            "  Gap:        {:.1} mil ({gap:.3} mm)",
            gap / 0.0254
        ));
    }
    out.print("");
    out.print("  Results:");
    out.print(&format!("    Z₀ = {:.1}Ω", result.z0));
    out.print(&format!("    εeff = {:.2}", result.epsilon_eff));
    out.print(&format!(
        "    Delay = {:.1} ps/mm ({:.0} ps/inch)",
        result.propagation_delay_ps_per_mm(),
        result.propagation_delay_ns_per_inch() * 1000.0
    ));
    out.print(&format!(
        "    Loss = {:.3} dB/inch @ {}GHz",
        result.loss_db_per_m * 0.0254,
        f(freq_ghz)
    ));
    Ok(0)
}

// ----------------------------------------------------------------- diffpair

fn run_diffpair(
    common: &Common,
    width_mm: f64,
    gap_mm: f64,
    layer: &str,
    target: Option<f64>,
    out: &mut Console,
) -> CmdResult {
    let stackup = get_stackup(common, out)?;
    let cl = CoupledLines::new(stackup.clone());
    let (result, geometry) = if stackup.is_outer_layer(layer) {
        (
            cl.edge_coupled_microstrip(width_mm, gap_mm, layer)?,
            "Edge-coupled microstrip",
        )
    } else {
        (
            cl.edge_coupled_stripline(width_mm, gap_mm, layer)?,
            "Edge-coupled stripline",
        )
    };
    let target = target.filter(|t| *t != 0.0);
    let target_check = target.map(|t| {
        let tolerance_pct = 5.0;
        let diff_pct = (result.zdiff - t).abs() / t * 100.0;
        if diff_pct <= tolerance_pct {
            (true, format!("✓ Within ±{tolerance_pct:.0}% of {}Ω", f(t)))
        } else {
            (false, format!("✗ {diff_pct:.1}% from {}Ω target", f(t)))
        }
    });

    if common.format == Format::Json {
        let mut fields = vec![
            (
                "geometry",
                Json::str(&geometry.to_lowercase().replace(' ', "_")),
            ),
            ("layer", Json::str(layer)),
            ("width_mm", Json::Num(width_mm)),
            ("gap_mm", Json::Num(gap_mm)),
            (
                "results",
                Json::obj(vec![
                    ("zdiff_ohm", Json::Num(py_round(result.zdiff, 2))),
                    ("zcommon_ohm", Json::Num(py_round(result.zcommon, 2))),
                    ("z0_even_ohm", Json::Num(py_round(result.z0_even, 2))),
                    ("z0_odd_ohm", Json::Num(py_round(result.z0_odd, 2))),
                    (
                        "coupling_coefficient",
                        Json::Num(py_round(result.coupling_coefficient, 3)),
                    ),
                ]),
            ),
        ];
        if let (Some(t), Some((pass, _))) = (target, &target_check) {
            fields.push(("target_zdiff_ohm", Json::Num(t)));
            fields.push(("target_met", Json::Bool(*pass)));
        }
        fields.push((
            "construction",
            construction_json(stackup.construction.as_ref()),
        ));
        out.raw(&(Json::obj(fields).dumps() + "\n"));
        return Ok(0);
    }

    out.print("\nDifferential Pair Analysis:\n");
    out.print(&format!("  Geometry:   {geometry}"));
    out.print(&format!(
        "  Width:      {:.1} mil ({width_mm:.3} mm) each",
        width_mm / 0.0254
    ));
    out.print(&format!(
        "  Gap:        {:.1} mil ({gap_mm:.3} mm)",
        gap_mm / 0.0254
    ));
    out.print(&format!("  Layer:      {layer}"));
    out.print("");
    out.print("  Results:");
    out.print_end(&format!("    Zdiff = {:.1}Ω", result.zdiff), "");
    match &target_check {
        Some((_, msg)) => out.print(&format!(" ({msg})")),
        None => out.print(""),
    }
    out.print(&format!("    Zcommon = {:.1}Ω", result.zcommon));
    out.print(&format!("    Z0_even = {:.1}Ω", result.z0_even));
    out.print(&format!("    Z0_odd = {:.1}Ω", result.z0_odd));
    out.print(&format!(
        "    Coupling k = {:.2}",
        result.coupling_coefficient
    ));
    out.print("");
    out.print("  Recommendations:");
    let z = result.zdiff;
    if (81.0..=99.0).contains(&z) {
        out.print("    ✓ Good for USB 2.0 (90Ω ±10%)");
    }
    if (90.0..=110.0).contains(&z) {
        out.print("    ✓ Good for LVDS (100Ω ±10%)");
    }
    if (85.0..=115.0).contains(&z) {
        out.print("    ✓ Good for HDMI (100Ω ±15%)");
    }
    Ok(0)
}

// ---------------------------------------------------------------- crosstalk

#[allow(clippy::too_many_arguments)]
fn run_crosstalk(
    common: &Common,
    layer: &str,
    width_mm: Option<f64>,
    spacing_mm: Option<f64>,
    length_mm: Option<f64>,
    rise_time_ns: f64,
    max_percent: Option<f64>,
    out: &mut Console,
) -> CmdResult {
    let stackup = get_stackup(common, out)?;
    let xt = CrosstalkAnalyzer::new(stackup.clone());
    let truthy = |v: Option<f64>| v.filter(|x| *x != 0.0);
    let (max_percent, length_mm, width_mm) =
        (truthy(max_percent), truthy(length_mm), truthy(width_mm));

    if let (Some(max_percent), Some(length_mm), Some(width_mm)) = (max_percent, length_mm, width_mm)
    {
        let spacing =
            xt.spacing_for_crosstalk_budget(max_percent, width_mm, length_mm, layer, rise_time_ns)?;
        if common.format == Format::Json {
            let data = Json::obj(vec![
                ("calculation", Json::str("spacing_for_budget")),
                ("max_crosstalk_percent", Json::Num(max_percent)),
                ("parallel_length_mm", Json::Num(length_mm)),
                ("width_mm", Json::Num(width_mm)),
                ("rise_time_ns", Json::Num(rise_time_ns)),
                ("layer", Json::str(layer)),
                (
                    "result",
                    Json::obj(vec![
                        ("minimum_spacing_mm", Json::Num(py_round(spacing, 3))),
                        (
                            "minimum_spacing_mil",
                            Json::Num(py_round(spacing / 0.0254, 1)),
                        ),
                    ]),
                ),
                (
                    "construction",
                    construction_json(stackup.construction.as_ref()),
                ),
            ]);
            out.raw(&(data.dumps() + "\n"));
            return Ok(0);
        }
        out.print(&format!("\nSpacing for <{}% Crosstalk:\n", f(max_percent)));
        out.print(&format!("  Parallel length: {} mm", f(length_mm)));
        out.print(&format!(
            "  Trace width:     {:.1} mil ({width_mm:.3} mm)",
            width_mm / 0.0254
        ));
        out.print(&format!("  Max crosstalk:   {}%", f(max_percent)));
        out.print(&format!("  Rise time:       {} ns", f(rise_time_ns)));
        out.print("");
        out.print(&format!(
            "  Minimum spacing: {:.1} mil ({spacing:.3} mm)",
            spacing / 0.0254
        ));
        return Ok(0);
    }

    let (Some(spacing_mm), Some(length_mm), Some(width_mm)) =
        (truthy(spacing_mm), length_mm, width_mm)
    else {
        out.print("Error: For crosstalk analysis, provide --spacing, --length, and --width");
        out.print("       Or use --max-percent, --length, and --width to calculate spacing");
        return Ok(1);
    };

    let result = xt.analyze(
        width_mm,
        width_mm,
        spacing_mm,
        length_mm,
        layer,
        rise_time_ns,
    )?;

    if common.format == Format::Json {
        let data = Json::obj(vec![
            ("spacing_mm", Json::Num(spacing_mm)),
            ("parallel_length_mm", Json::Num(length_mm)),
            ("width_mm", Json::Num(width_mm)),
            ("layer", Json::str(layer)),
            ("rise_time_ns", Json::Num(rise_time_ns)),
            (
                "results",
                Json::obj(vec![
                    ("next_percent", Json::Num(py_round(result.next_percent, 2))),
                    ("next_db", Json::Num(py_round(result.next_db, 1))),
                    ("fext_percent", Json::Num(py_round(result.fext_percent, 2))),
                    ("fext_db", Json::Num(py_round(result.fext_db, 1))),
                    (
                        "saturation_length_mm",
                        Json::Num(py_round(result.saturation_length_mm, 2)),
                    ),
                ]),
            ),
            ("severity", Json::str(&result.severity)),
            (
                "recommendation",
                result
                    .recommendation
                    .as_deref()
                    .map_or(Json::Null, Json::str),
            ),
            (
                "construction",
                construction_json(stackup.construction.as_ref()),
            ),
        ]);
        out.raw(&(data.dumps() + "\n"));
        return Ok(0);
    }

    out.print("\nCrosstalk Analysis:\n");
    out.print(&format!(
        "  Spacing:        {:.1} mil ({spacing_mm:.3} mm)",
        spacing_mm / 0.0254
    ));
    out.print(&format!("  Parallel:       {} mm", f(length_mm)));
    out.print(&format!(
        "  Width:          {:.1} mil ({width_mm:.3} mm) each",
        width_mm / 0.0254
    ));
    out.print(&format!("  Layer:          {layer}"));
    out.print(&format!("  Rise time:      {} ns", f(rise_time_ns)));
    out.print("");
    out.print("  Results:");
    out.print(&format!(
        "    NEXT = {:.1}% ({:.1} dB)",
        result.next_percent, result.next_db
    ));
    out.print(&format!(
        "    FEXT = {:.1}% ({:.1} dB)",
        result.fext_percent, result.fext_db
    ));
    out.print(&format!(
        "    Saturation length: {:.1} mm",
        result.saturation_length_mm
    ));
    out.print("");
    out.print(&format!(
        "  Crosstalk Risk: {}",
        result.severity.to_uppercase()
    ));
    if let Some(rec) = &result.recommendation {
        out.print("");
        out.print(&format!("  Recommendation: {rec}"));
    }
    Ok(0)
}

// ------------------------------------------------------------------- output

/// Minimal stand-in for a Rich `Console` writing to a non-terminal:
/// 80-column word wrap per `print` call, no styling.
#[derive(Default)]
struct Console {
    buf: String,
}

const CONSOLE_WIDTH: usize = 80;

impl Console {
    fn raw(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    fn print(&mut self, s: &str) {
        self.print_end(s, "\n");
    }

    fn print_end(&mut self, s: &str, end: &str) {
        let lines: Vec<String> = s
            .split('\n')
            .flat_map(|l| wrap_line(l, CONSOLE_WIDTH))
            .collect();
        self.buf.push_str(&lines.join("\n"));
        self.buf.push_str(end);
    }

    fn take(&mut self) -> String {
        std::mem::take(&mut self.buf)
    }
}

fn cell_len(s: &str) -> usize {
    s.chars().count()
}

/// Rich `divide_line` word wrapping: words keep trailing whitespace; a word
/// that does not fit starts a new line; over-long words are folded.
fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if cell_len(line) <= width {
        return vec![line.to_string()];
    }
    // Split into `\s*\S+\s*` words.
    let chars: Vec<char> = line.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        words.push(chars[start..i].iter().collect());
    }
    let mut lines: Vec<String> = vec![String::new()];
    let mut offset = 0;
    for word in words {
        let word_len = cell_len(word.trim_end());
        let remaining = width.saturating_sub(offset);
        if remaining >= word_len {
            lines.last_mut().unwrap().push_str(&word);
            offset += cell_len(&word);
        } else if word_len > width {
            // Fold: fill the current line then chunk the rest.
            let wchars: Vec<char> = word.chars().collect();
            let mut pos = 0;
            if offset > 0 {
                lines.push(String::new());
            }
            while pos < wchars.len() {
                let end = (pos + width).min(wchars.len());
                let chunk: String = wchars[pos..end].iter().collect();
                if !lines.last().unwrap().is_empty() {
                    lines.push(String::new());
                }
                lines.last_mut().unwrap().push_str(&chunk);
                offset = end - pos;
                pos = end;
            }
        } else {
            lines.push(word.clone());
            offset = cell_len(&word);
        }
    }
    // Trailing whitespace is only trimmed beyond the width.
    lines
        .into_iter()
        .map(|l| {
            if cell_len(&l) > width {
                l.chars().take(width).collect()
            } else {
                l
            }
        })
        .collect()
}

/// Rich default table (`box.HEAVY_HEAD`, bold header, 1-space padding).
fn render_table(headers: &[(&str, bool)], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, (h, _))| {
            rows.iter()
                .map(|r| cell_len(&r[i]))
                .chain(std::iter::once(cell_len(h)))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let rule = |l: &str, fill: &str, mid: &str, r: &str| -> String {
        let segs: Vec<String> = widths.iter().map(|w| fill.repeat(w + 2)).collect();
        format!("{l}{}{r}\n", segs.join(mid))
    };
    let row = |cells: Vec<&str>, sep: &str| -> String {
        let segs: Vec<String> = cells
            .iter()
            .zip(&widths)
            .zip(headers)
            .map(|((c, w), (_, right))| {
                let pad = w - cell_len(c);
                if *right {
                    format!(" {}{c} ", " ".repeat(pad))
                } else {
                    format!(" {c}{} ", " ".repeat(pad))
                }
            })
            .collect();
        format!("{sep}{}{sep}\n", segs.join(sep))
    };
    let mut out = rule("┏", "━", "┳", "┓");
    out += &row(headers.iter().map(|(h, _)| *h).collect(), "┃");
    out += &rule("┡", "━", "╇", "┩");
    for r in rows {
        out += &row(r.iter().map(String::as_str).collect(), "│");
    }
    out += &rule("└", "─", "┴", "┘");
    out
}

// --------------------------------------------------------------------- json

/// Ordered JSON value serialized like Python `json.dumps(obj, indent=2)`.
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }

    fn obj(fields: Vec<(&str, Json)>) -> Json {
        Json::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    fn dumps(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, indent: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => out.push_str(&i.to_string()),
            Json::Num(x) => out.push_str(&if x.is_nan() {
                "NaN".to_string()
            } else if x.is_infinite() {
                if *x > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
            } else {
                py_float_repr(*x)
            }),
            Json::Str(s) => write_json_str(out, s),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    out.push_str(&" ".repeat(indent + 2));
                    item.write(out, indent + 2);
                }
                out.push('\n');
                out.push_str(&" ".repeat(indent));
                out.push(']');
            }
            Json::Obj(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    out.push_str(&" ".repeat(indent + 2));
                    write_json_str(out, k);
                    out.push_str(": ");
                    v.write(out, indent + 2);
                }
                out.push('\n');
                out.push_str(&" ".repeat(indent));
                out.push('}');
            }
        }
    }
}

/// `ensure_ascii=True` string escaping.
fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn construction_json(c: Option<&Construction>) -> Json {
    match c {
        None => Json::Null,
        Some(Construction::Legacy { id }) => Json::obj(vec![
            ("id", Json::str(id)),
            ("factory_id", Json::Null),
            ("compatibility_only", Json::Bool(true)),
        ]),
        Some(Construction::Factory {
            id,
            manufacturer,
            source_url,
            verified_on,
            nominal_board_thickness_mm,
            outer_copper_oz,
            inner_copper_oz,
            assumptions,
        }) => Json::obj(vec![
            ("id", Json::str(id)),
            ("factory_id", Json::str(id)),
            ("manufacturer", Json::str(manufacturer)),
            ("source_url", Json::str(source_url)),
            ("verified_on", Json::str(verified_on)),
            (
                "nominal_board_thickness_mm",
                Json::Num(*nominal_board_thickness_mm),
            ),
            ("outer_copper_oz", Json::Num(*outer_copper_oz)),
            ("inner_copper_oz", Json::Num(*inner_copper_oz)),
            (
                "assumptions",
                Json::Arr(assumptions.iter().map(|a| Json::str(a)).collect()),
            ),
        ]),
    }
}

fn opt_num(v: Option<f64>) -> Json {
    v.map_or(Json::Null, Json::Num)
}

fn summary_json(s: &StackupSummary) -> Json {
    Json::obj(vec![
        ("construction", construction_json(s.construction.as_ref())),
        ("board_thickness_mm", Json::Num(s.board_thickness_mm)),
        ("num_copper_layers", Json::Int(s.num_copper_layers as i64)),
        ("copper_finish", Json::str(&s.copper_finish)),
        (
            "layers",
            Json::Arr(
                s.layers
                    .iter()
                    .map(|l| {
                        Json::obj(vec![
                            ("name", Json::str(&l.name)),
                            ("type", Json::str(l.layer_type)),
                            ("thickness_mm", Json::Num(l.thickness_mm)),
                            ("material", Json::str(&l.material)),
                            ("epsilon_r", opt_num(l.epsilon_r)),
                            ("copper_oz", opt_num(l.copper_oz)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_like_rich() {
        let line = "  Recommendation: Increase spacing to 0.20mm or more; Consider routing on different layers with ground between";
        assert_eq!(
            wrap_line(line, 80),
            vec![
                "  Recommendation: Increase spacing to 0.20mm or more; Consider routing on "
                    .to_string(),
                "different layers with ground between".to_string()
            ]
        );
    }

    #[test]
    fn json_matches_python_dumps() {
        let j = Json::obj(vec![
            ("a", Json::Num(50.0)),
            ("b", Json::Arr(vec![])),
            (
                "c",
                Json::obj(vec![("d", Json::Null), ("e", Json::str("μ"))]),
            ),
        ]);
        assert_eq!(
            j.dumps(),
            "{\n  \"a\": 50.0,\n  \"b\": [],\n  \"c\": {\n    \"d\": null,\n    \"e\": \"\\u03bc\"\n  }\n}"
        );
    }
}
