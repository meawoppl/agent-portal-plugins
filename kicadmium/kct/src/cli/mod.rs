//! `kct` command-line entry point (port of `kicad_tools.cli`).
//!
//! Every upstream top-level command is listed in [`COMMANDS`]. A ported
//! command sets `run`; unported ones fail loudly with exit code 3. Each command
//! module owns its argument parsing (clap) so waves can land independently:
//! add `pub mod <cmd>;` below and point its registry row at `<cmd>::run`.

use std::ffi::OsString;
use std::io::Write;

use anyhow::Result;

use crate::units::{self, UnitFormatter};

pub mod analyze;
pub mod audit;
pub mod board_metrics;
pub mod check;
pub mod constraints;
pub mod creepage;
pub mod creepage_export_rules;
pub mod datasheet;
pub mod decisions;
pub mod design;
pub mod detect_mistakes;
pub mod drc;
pub mod erc;
pub mod estimate;
pub mod explain;
pub mod fabrication;
pub mod fleet;
pub mod footprint;
pub mod h;
pub mod impedance;
pub mod mfr;
pub mod net_status;
pub mod optim;
pub mod optimize_placement;
pub mod parts;
pub mod pcb;
pub mod placement;
pub mod project;
pub mod render;
pub mod report;
pub mod runner;
pub mod screenshot;
pub mod stitch;
pub mod suggest;
pub mod utility;
pub mod workflow;
pub mod zones;

/// Global flags accepted before the command name.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    pub verbose: bool,
    pub quiet: bool,
    pub units: UnitFormatter,
}

/// Command entry: args exclude the command name itself.
pub type RunFn = fn(Vec<OsString>, &Globals) -> Result<i32>;

pub struct CommandSpec {
    pub name: &'static str,
    pub about: &'static str,
    /// Port wave owning the command (see PORTING.md).
    pub wave: &'static str,
    pub run: Option<RunFn>,
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec { name: "symbols", about: "List symbols in a schematic", wave: "B", run: Some(design::symbols) },
    CommandSpec { name: "nets", about: "Trace nets in a schematic", wave: "B", run: Some(design::nets) },
    CommandSpec { name: "netlist", about: "Netlist analysis and comparison tools", wave: "B", run: Some(design::netlist) },
    CommandSpec { name: "erc", about: "ERC validation and analysis", wave: "A", run: Some(erc::run) },
    CommandSpec { name: "drc", about: "Parse DRC report", wave: "A", run: Some(drc::run) },
    CommandSpec { name: "bom", about: "Generate bill of materials", wave: "B", run: Some(design::bom) },
    CommandSpec { name: "check", about: "Pure Python DRC (no kicad-cli)", wave: "A", run: Some(check::run) },
    CommandSpec { name: "creepage", about: "HV creepage/clearance census (surface-path distance)", wave: "E", run: Some(creepage::run) },
    CommandSpec { name: "creepage-export-rules", about: "Export voltage-domain netclasses + pairwise HV clearance (rule) clauses so kicad-cli DRC enforces creepage (Issue #4508)", wave: "E", run: Some(creepage_export_rules::run) },
    CommandSpec { name: "sch", about: "Schematic analysis tools", wave: "B", run: Some(design::sch) },
    CommandSpec { name: "pcb", about: "PCB query tools", wave: "C", run: Some(pcb::run) },
    CommandSpec { name: "lib", about: "Symbol and footprint library tools", wave: "B", run: Some(design::lib) },
    CommandSpec { name: "footprint", about: "Footprint generation and tools", wave: "G", run: Some(footprint::run) },
    CommandSpec { name: "mfr", about: "Manufacturer tools", wave: "G", run: Some(mfr::run) },
    CommandSpec { name: "zones", about: "Add copper pour zones to PCB", wave: "E", run: Some(zones::run) },
    CommandSpec { name: "stitch", about: "Auto-add stitching vias for plane connections", wave: "E", run: Some(stitch::run) },
    CommandSpec { name: "route", about: "Autoroute a PCB", wave: "F", run: None },
    CommandSpec { name: "route-auto", about: "Route a net using RoutingOrchestrator smart strategy selection", wave: "F", run: None },
    CommandSpec { name: "reason", about: "LLM-driven PCB layout reasoning", wave: "H", run: Some(h::reason) },
    CommandSpec { name: "optimize-traces", about: "Optimize PCB traces", wave: "D", run: None },
    CommandSpec { name: "validate-footprints", about: "Validate footprints for pad spacing issues", wave: "D", run: None },
    CommandSpec { name: "fix-footprints", about: "Fix footprint pad spacing issues", wave: "D", run: None },
    CommandSpec { name: "fix-vias", about: "Fix vias to meet manufacturer specifications", wave: "D", run: None },
    CommandSpec { name: "fix-silkscreen", about: "Fix silkscreen line widths to meet manufacturer specifications", wave: "D", run: None },
    CommandSpec { name: "place-silk-refs", about: "Move readable silkscreen reference designators to clear collisions", wave: "D", run: None },
    CommandSpec { name: "repair-clearance", about: "Repair clearance violations by nudging traces", wave: "D", run: None },
    CommandSpec { name: "fix-drc", about: "Automated DRC violation repair (clearance + drill)", wave: "D", run: None },
    CommandSpec { name: "fix-erc", about: "Automated ERC violation repair (PWR_FLAG + no-connect)", wave: "D", run: None },
    CommandSpec { name: "parts", about: "LCSC parts lookup and search", wave: "G", run: Some(parts::run) },
    CommandSpec { name: "datasheet", about: "Datasheet search, download, and PDF parsing", wave: "G", run: Some(datasheet::run) },
    CommandSpec { name: "decisions", about: "Query design decisions (placement and routing rationale)", wave: "E", run: Some(decisions::run) },
    CommandSpec { name: "placement", about: "Detect and fix placement conflicts", wave: "E", run: Some(placement::run) },
    CommandSpec { name: "optimize-placement", about: "Run CMA-ES placement optimization on a KiCad PCB", wave: "E", run: Some(optimize_placement::run) },
    CommandSpec { name: "config", about: "View and manage configuration", wave: "G", run: Some(utility::config) },
    CommandSpec { name: "interactive", about: "Launch interactive REPL mode", wave: "H", run: Some(h::interactive) },
    CommandSpec { name: "validate", about: "Validation tools", wave: "B", run: Some(design::validate) },
    CommandSpec { name: "analyze", about: "PCB analysis tools", wave: "C", run: Some(analyze::run) },
    CommandSpec { name: "constraints", about: "Constraint conflict detection and management", wave: "E", run: Some(constraints::run) },
    CommandSpec { name: "estimate", about: "Manufacturing cost estimation", wave: "C", run: Some(estimate::run) },
    CommandSpec { name: "audit", about: "Manufacturing readiness audit (ERC, DRC, connectivity, compatibility)", wave: "C", run: Some(audit::audit) },
    CommandSpec { name: "suggest", about: "Part suggestions and recommendations", wave: "G", run: Some(suggest::run) },
    CommandSpec { name: "net-status", about: "Report net connectivity status for a PCB", wave: "C", run: Some(net_status::run) },
    CommandSpec { name: "fleet", about: "Fleet-wide PCB status and operations", wave: "C", run: Some(fleet::run) },
    CommandSpec { name: "render", about: "Render per-board 2D SVGs + 3D PNGs into output/renders/", wave: "C", run: Some(render::run) },
    CommandSpec { name: "board-metrics", about: "Emit a normalized board.json per board from existing artifacts", wave: "C", run: Some(board_metrics::run) },
    CommandSpec { name: "readiness", about: "Run the manufacturing-readiness gates and write output/readiness.json", wave: "C", run: Some(audit::readiness) },
    CommandSpec { name: "clean", about: "Clean up old/orphaned files from KiCad projects", wave: "G", run: Some(utility::clean) },
    CommandSpec { name: "impedance", about: "Transmission line impedance calculations", wave: "E", run: Some(impedance::run) },
    CommandSpec { name: "ipc", about: "Interact with a running KiCad instance via IPC API (KiCad 9.0+)", wave: "H", run: Some(h::ipc) },
    CommandSpec { name: "init", about: "Initialize a KiCad project with manufacturer design rules", wave: "G", run: Some(project::init) },
    CommandSpec { name: "panel", about: "Create manufacturing panels from board PCBs", wave: "G", run: Some(fabrication::panel) },
    CommandSpec { name: "pipeline", about: "End-to-end repair pipeline for existing PCBs", wave: "G", run: Some(workflow::pipeline) },
    CommandSpec { name: "create-pcb", about: "Create a PCB from a KiCad schematic", wave: "G", run: Some(fabrication::create_pcb) },
    CommandSpec { name: "build", about: "Build from spec to manufacturable design", wave: "G", run: Some(workflow::build) },
    CommandSpec { name: "doctor", about: "Diagnose kicad-tools installation health (version-record drift + environment preflight)", wave: "G", run: Some(utility::doctor) },
    CommandSpec { name: "spec", about: "Project specification (.kct) management", wave: "G", run: Some(project::spec) },
    CommandSpec { name: "benchmark", about: "Run routing benchmarks and regression tests", wave: "F", run: None },
    CommandSpec { name: "bench", about: "Run external DeepPCB-comparable board benchmarks", wave: "F", run: None },
    CommandSpec { name: "sync", about: "Reconcile schematic and PCB references", wave: "B", run: Some(design::sync) },
    CommandSpec { name: "run", about: "Run a native kct JSON/YAML automation workflow", wave: "H", run: Some(h::run) },
    CommandSpec { name: "explain", about: "Explain design rules and DRC violations", wave: "A", run: Some(explain::run) },
    CommandSpec { name: "detect-mistakes", about: "Detect common PCB design mistakes with educational explanations", wave: "A", run: Some(detect_mistakes::run) },
    CommandSpec { name: "calibrate", about: "Calibrate routing performance settings for your machine", wave: "F", run: None },
    CommandSpec { name: "screenshot", about: "Capture a PNG screenshot of a KiCad board or schematic", wave: "C", run: Some(screenshot::run) },
    CommandSpec { name: "report", about: "Generate a Markdown design report", wave: "C", run: Some(report::run) },
    CommandSpec { name: "export", about: "Generate a complete manufacturing package (BOM, CPL, Gerbers, project ZIP, manifest)", wave: "G", run: Some(fabrication::export) },
    CommandSpec { name: "optim", about: "Placement / routing FOM tools (issue #3186)", wave: "E", run: Some(optim::run) },
];

/// Exit code for commands that are not ported yet.
pub const EXIT_NOT_PORTED: i32 = 3;

fn usage() -> String {
    let mut out = format!(
        "usage: kct [--verbose] [-q] [--units {{mm,mils}}] <command> [args...]\n\n\
         KiCad automation toolkit (Rust port of kicad-tools {})\n\ncommands:\n",
        crate::UPSTREAM_VERSION
    );
    for c in COMMANDS {
        let mark = if c.run.is_some() {
            ""
        } else {
            "  [not yet ported]"
        };
        out.push_str(&format!("  {:<22} {}{mark}\n", c.name, c.about));
    }
    out
}

/// Run `kct <args...>` (argv[0] excluded). Returns the process exit code.
pub fn run<I, T>(args: I) -> Result<i32>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let mut globals = Globals::default();
    let mut units_flag: Option<String> = None;
    while let Some(first) = args.first().and_then(|a| a.to_str()).map(str::to_owned) {
        match first.as_str() {
            "--verbose" => globals.verbose = true,
            "-q" | "--quiet" => globals.quiet = true,
            "--units" => {
                args.remove(0);
                units_flag = args.first().and_then(|a| a.to_str()).map(str::to_owned);
                if units_flag.is_none() {
                    anyhow::bail!("--units requires mm or mils");
                }
            }
            s if s.starts_with("--units=") => units_flag = Some(s["--units=".len()..].to_owned()),
            "--version" => {
                println!("kct {} (kicadmium port)", crate::UPSTREAM_VERSION);
                return Ok(0);
            }
            "-h" | "--help" => {
                print!("{}", usage());
                return Ok(0);
            }
            _ => break,
        }
        args.remove(0);
    }
    globals.units = UnitFormatter::resolve(units_flag.as_deref());
    units::set_current(globals.units);

    let Some(name) = args.first().and_then(|a| a.to_str()).map(str::to_owned) else {
        eprint!("{}", usage());
        return Ok(2);
    };
    let rest = args.split_off(1);
    let Some(spec) = COMMANDS.iter().find(|c| c.name == name) else {
        eprintln!("kct: unknown command '{name}'\n");
        eprint!("{}", usage());
        return Ok(2);
    };
    match spec.run {
        Some(run) => {
            let code = run(rest, &globals)?;
            std::io::stdout().flush().ok();
            Ok(code)
        }
        None => {
            eprintln!(
                "kct {name}: not yet ported to Rust (wave {}; see kct/PORTING.md)",
                spec.wave
            );
            Ok(EXIT_NOT_PORTED)
        }
    }
}

/// Parse a command's own flags with clap, prefixing the command name so help
/// and errors read `kct <name> ...`. Help/usage errors print and exit.
pub fn parse_args<P: clap::Parser>(name: &str, args: Vec<OsString>) -> P {
    let argv = std::iter::once(OsString::from(format!("kct {name}"))).chain(args);
    P::parse_from(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_names_are_unique() {
        let mut names: Vec<_> = COMMANDS.iter().map(|c| c.name).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(len, names.len());
    }

    #[test]
    fn unported_commands_exit_3() {
        let unported = COMMANDS.iter().find(|c| c.run.is_none()).map(|c| c.name);
        if let Some(name) = unported {
            assert_eq!(run([name]).unwrap(), EXIT_NOT_PORTED);
        }
    }
}
