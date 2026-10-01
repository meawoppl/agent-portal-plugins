//! argparse-compatible option parsing for the routing commands (`route`,
//! `route-auto`, `calibrate`, `benchmark`, `bench`).
//!
//! Upstream defines ~130 `kct route` options; this table-driven parser keeps
//! every upstream flag name (unambiguous long-option prefixes are accepted,
//! like argparse), its value arity and its default, and reports usage errors
//! with exit code 2.

use std::collections::HashMap;
use std::ffi::OsString;

/// One option: names (first long name is canonical), dest, takes a value,
/// default (None = Python None), boolean store value for flags.
#[derive(Debug, Clone, Copy)]
pub struct Opt {
    pub names: &'static [&'static str],
    pub dest: &'static str,
    pub value: bool,
    pub default: Option<&'static str>,
    /// For flags: value stored when present ("true"/"false").
    pub store: &'static str,
    pub choices: &'static [&'static str],
}

const fn v(names: &'static [&'static str], dest: &'static str, default: Option<&'static str>) -> Opt {
    Opt {
        names,
        dest,
        value: true,
        default,
        store: "",
        choices: &[],
    }
}

const fn c(
    names: &'static [&'static str],
    dest: &'static str,
    default: Option<&'static str>,
    choices: &'static [&'static str],
) -> Opt {
    Opt {
        names,
        dest,
        value: true,
        default,
        store: "",
        choices,
    }
}

const fn f(names: &'static [&'static str], dest: &'static str, default: &'static str, store: &'static str) -> Opt {
    Opt {
        names,
        dest,
        value: false,
        default: Some(default),
        store,
        choices: &[],
    }
}

/// `kct route` options (upstream `cli/parser.py` route subparser).
pub static ROUTE_OPTS: &[Opt] = &[
    v(&["-o", "--output"], "output", None),
    c(&["--format"], "format", Some("text"), &["text", "json"]),
    c(&["--strategy"], "strategy", Some("negotiated"), &["basic", "negotiated", "monte-carlo", "evolutionary"]),
    v(&["--skip-nets"], "skip_nets", None),
    v(&["--nets"], "nets", None),
    f(&["--preserve-existing"], "preserve_existing", "false", "true"),
    f(&["--complete"], "complete", "false", "true"),
    v(&["--complete-exclude-nets"], "complete_exclude_nets", Some("")),
    v(&["--complete-report"], "complete_report", None),
    v(&["--region"], "region", None),
    v(&["--grid"], "grid", Some("auto")),
    v(&["--max-cells"], "max_cells", Some("500000")),
    v(&["--trace-width"], "trace_width", None),
    v(&["--clearance"], "clearance", Some("0.15")),
    f(&["--strict-pad-clearance"], "strict_pad_clearance", "false", "true"),
    v(&["--fine-pitch-clearance"], "fine_pitch_clearance", None),
    v(&["--via-drill"], "via_drill", Some("0.3")),
    v(&["--via-diameter"], "via_diameter", Some("0.6")),
    v(&["--mc-trials"], "mc_trials", Some("10")),
    v(&["--pop-size"], "pop_size", Some("20")),
    v(&["--generations"], "generations", Some("10")),
    v(&["--iterations"], "iterations", Some("15")),
    v(&["--early-stop-patience"], "early_stop_patience", Some("2")),
    f(&["--targeted-ripup"], "targeted_ripup", "false", "true"),
    v(&["--max-ripups-per-net"], "max_ripups_per_net", None),
    f(&["--bundle-river-planner"], "bundle_river_planner", "false", "true"),
    f(&["--monotone-certificate-order"], "monotone_certificate_order", "false", "true"),
    f(&["--no-rescue-pass"], "no_rescue_pass", "false", "true"),
    f(&["--no-routing-plan"], "routing_plan", "true", "false"),
    f(&["--plan-gate"], "plan_gate", "false", "true"),
    f(&["--cross-package-pair-corridor"], "cross_package_pair_corridor", "false", "true"),
    f(&["--slack-corridor-widening"], "slack_corridor_widening", "false", "true"),
    f(&["--escape-corridor-reservation"], "escape_corridor_reservation", "false", "true"),
    f(&["--region-parallel"], "region_parallel", "false", "true"),
    v(&["--partition-rows"], "partition_rows", Some("2")),
    v(&["--partition-cols"], "partition_cols", Some("2")),
    v(&["--max-parallel-workers"], "max_parallel_workers", Some("4")),
    v(&["--timeout"], "timeout", None),
    v(&["--search-timeout"], "search_timeout", None),
    v(&["--per-net-timeout"], "per_net_timeout", Some("30.0")),
    v(&["--checkpoint-interval"], "checkpoint_interval", Some("30.0")),
    v(&["--max-search-iterations"], "max_search_iterations", Some("0")),
    v(&["--per-net-iterations"], "per_net_iterations", Some("0")),
    f(&["--deterministic-budget"], "deterministic_budget", "false", "true"),
    f(&["-v", "--verbose"], "verbose", "false", "true"),
    f(&["--dry-run"], "dry_run", "false", "true"),
    f(&["-q", "--quiet"], "quiet", "false", "true"),
    v(&["--power-nets"], "power_nets", None),
    c(&["--layers"], "layers", Some("auto"), &["auto", "2", "4", "4-sig", "4-all", "6"]),
    f(&["--force"], "force", "false", "true"),
    f(&["--allow-unsafe-grid"], "allow_unsafe_grid", "false", "true"),
    f(&["--no-optimize", "--raw"], "no_optimize", "false", "true"),
    f(&["--report-stage-quality"], "report_stage_quality", "false", "true"),
    f(&["--auto-pour"], "auto_pour", "none", "true"),
    f(&["--no-auto-pour"], "auto_pour", "none", "false"),
    f(&["--auto-layers"], "auto_layers", "none", "true"),
    f(&["--no-auto-layers"], "auto_layers", "none", "false"),
    c(&["--max-layers"], "max_layers", Some("6"), &["2", "4", "6"]),
    c(&["--starting-layers"], "starting_layers", None, &["2", "4", "6"]),
    v(&["--min-completion"], "min_completion", Some("0.95")),
    f(&["--adaptive-rules"], "adaptive_rules", "false", "true"),
    f(&["--auto-mfr-tier"], "auto_mfr_tier", "false", "true"),
    f(&["--auto-pcb-size"], "auto_pcb_size", "false", "true"),
    v(&["--packing-overhead"], "packing_overhead", None),
    v(&["--mfr-tier-ladder"], "mfr_tier_ladder", None),
    v(&["--min-trace"], "min_trace", None),
    v(&["--min-clearance-floor"], "min_clearance_floor", None),
    v(&["--manufacturer", "--mfr"], "manufacturer", Some("jlcpcb")),
    v(&["--copper"], "copper", None),
    f(&["--high-performance"], "high_performance", "false", "true"),
    f(&["--skip-drc"], "skip_drc", "false", "true"),
    f(&["--sync-check"], "sync_check", "true", "true"),
    f(&["--no-sync-check"], "sync_check", "true", "false"),
    v(&["--schematic"], "schematic", None),
    f(&["--allow-offboard"], "allow_offboard", "false", "true"),
    v(&["--census-advisory"], "census_advisory", None),
    f(&["--census-advisory-gate"], "census_advisory_gate", "false", "true"),
    v(&["--census-advisory-gate-pct"], "census_advisory_gate_pct", None),
    f(&["--capacity-forecast"], "capacity_forecast", "false", "true"),
    v(&["--capacity-forecast-json"], "capacity_forecast_json", None),
    f(&["--strict-drc"], "strict_drc", "false", "true"),
    f(&["--strict-layers"], "strict_layers", "false", "true"),
    f(&["--reserve-plane-layers"], "reserve_plane_layers", "true", "true"),
    f(&["--no-reserve-plane-layers"], "reserve_plane_layers", "true", "false"),
    f(&["--auto-fix"], "auto_fix", "false", "true"),
    v(&["--auto-fix-passes"], "auto_fix_passes", None),
    f(&["--placement-feedback"], "placement_feedback", "false", "true"),
    f(&["--no-placement-feedback"], "placement_feedback", "false", "false"),
    v(&["--placement-feedback-budget"], "placement_feedback_budget", Some("3")),
    v(&["--placement-feedback-max-movement"], "placement_feedback_max_movement", Some("5.0")),
    v(&["--placement-feedback-anchor"], "placement_feedback_anchor", None),
    v(&["--placement-feedback-no-anchor"], "placement_feedback_no_anchor", None),
    v(&["--placement-feedback-stagnation-patience"], "placement_feedback_stagnation_patience", Some("3")),
    v(&["--placement-feedback-outer-timeout"], "placement_feedback_outer_timeout", None),
    f(&["--placement-delta-feedback"], "placement_delta_feedback", "false", "true"),
    f(&["--no-placement-delta-feedback"], "placement_delta_feedback", "false", "false"),
    v(&["--placement-delta-feedback-budget"], "placement_delta_feedback_budget", Some("3")),
    v(&["--placement-delta-feedback-timeout"], "placement_delta_feedback_timeout", None),
    v(&["--export-failed-nets"], "export_failed_nets", None),
    f(&["--no-cache"], "no_cache", "false", "true"),
    c(&["--backend"], "backend", Some("auto"), &["auto", "cpp", "python"]),
    c(&["--route-engine"], "route_engine", Some("grid"), &["grid", "mesh", "lattice"]),
    f(&["--lattice-optimize"], "lattice_optimize", "false", "true"),
    v(&["--seed"], "seed", None),
    c(&["--order-method"], "order_method", None, &["greedy", "critical_first", "congestion", "hybrid"]),
    f(&["--no-auto-build-native"], "no_auto_build_native", "false", "true"),
    f(&["--strict"], "strict", "false", "true"),
    f(&["--strict-in-pad-clearance"], "route_strict_in_pad_clearance", "false", "true"),
    f(&["--micro-via-in-pad-fallback"], "route_micro_via_in_pad_fallback", "false", "true"),
    v(&["--micro-via-size"], "route_micro_via_size", Some("0.3")),
    v(&["--micro-via-drill"], "route_micro_via_drill", Some("0.15")),
    f(&["--via-in-pad-last-resort"], "route_via_in_pad_last_resort", "false", "true"),
    f(&["--differential-pairs"], "differential_pairs", "false", "true"),
    v(&["--diffpair-spacing"], "diffpair_spacing", None),
    v(&["--diffpair-max-delta"], "diffpair_max_delta", None),
    v(&["--diffpair-per-pair-timeout"], "diffpair_per_pair_timeout", None),
    f(&["--length-match-diffpairs"], "length_match_diffpairs", "false", "true"),
    f(&["--length-match-groups"], "length_match_groups", "false", "true"),
    v(&["--net-class-map"], "net_class_map", None),
    v(&["--current-paths"], "current_paths", None),
    f(&["--no-current-paths"], "no_current_paths", "false", "true"),
    v(&["--analog-nets"], "analog_nets", None),
    f(&["--auto-analog"], "auto_analog", "false", "true"),
    v(&["--voltage-map"], "voltage_map", None),
    c(&["--creepage-standard"], "creepage_standard", Some("iec60664"), &["iec60664", "iec62368"]),
    c(&["--pollution-degree"], "pollution_degree", Some("2"), &["1", "2", "3"]),
    v(&["--material-group"], "material_group", Some("IIIa")),
    v(&["--hv-threshold"], "hv_threshold", Some("30.0")),
    // Inner route_cmd parser options reachable from library callers.
    v(&["--edge-clearance"], "edge_clearance", None),
];

/// `kct route-auto` options.
pub static ROUTE_AUTO_OPTS: &[Opt] = &[
    v(&["--net"], "net", None),
    v(&["--nets"], "nets", None),
    v(&["--region"], "region", None),
    c(&["--strategy"], "strategy", Some("auto"), &["auto", "global", "escape", "hierarchical", "subgrid", "via_resolution"]),
    f(&["--allow-partial"], "allow_partial", "false", "true"),
    f(&["--no-repair"], "no_repair", "false", "true"),
    f(&["--no-via-resolution"], "no_via_resolution", "false", "true"),
    v(&["--via-drill"], "via_drill", None),
    v(&["--via-diameter"], "via_diameter", None),
    v(&["-o", "--output"], "output", None),
    f(&["--in-place"], "in_place", "false", "true"),
    f(&["--dry-run"], "dry_run", "false", "true"),
    f(&["-v", "--verbose"], "verbose", "false", "true"),
    c(&["--format"], "format", Some("text"), &["text", "json"]),
];

/// `kct calibrate` options.
pub static CALIBRATE_OPTS: &[Opt] = &[
    f(&["--show"], "calibrate_show", "false", "true"),
    f(&["--show-gpu"], "calibrate_show_gpu", "false", "true"),
    f(&["--gpu"], "calibrate_gpu", "false", "true"),
    f(&["--all"], "calibrate_all", "false", "true"),
    f(&["--benchmark"], "calibrate_benchmark", "false", "true"),
    f(&["--quick"], "calibrate_quick", "false", "true"),
    v(&["-o", "--output"], "calibrate_output", None),
    c(&["--format"], "calibrate_format", Some("text"), &["text", "json"]),
    f(&["--json"], "calibrate_json", "false", "true"),
    f(&["-v", "--verbose"], "calibrate_verbose", "false", "true"),
];

/// Parsed namespace (Python `argparse.Namespace`).
#[derive(Debug, Clone, Default)]
pub struct Namespace {
    pub values: HashMap<String, Option<String>>,
    pub positionals: Vec<String>,
    /// Option strings the user actually typed (canonical names).
    pub explicit: Vec<String>,
}

impl Namespace {
    pub fn get(&self, dest: &str) -> Option<&str> {
        self.values.get(dest).and_then(|v| v.as_deref())
    }

    pub fn str_or(&self, dest: &str, default: &str) -> String {
        self.get(dest).unwrap_or(default).to_string()
    }

    pub fn flag(&self, dest: &str) -> bool {
        self.get(dest) == Some("true")
    }

    /// Tri-state flag (`None` when the user passed neither form).
    pub fn tri(&self, dest: &str) -> Option<bool> {
        match self.get(dest) {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        }
    }

    pub fn f64(&self, dest: &str) -> Option<f64> {
        self.get(dest).and_then(|s| s.parse().ok())
    }

    pub fn usize(&self, dest: &str) -> Option<usize> {
        self.get(dest).and_then(|s| s.parse().ok())
    }

    pub fn set(&mut self, dest: &str, value: Option<&str>) {
        self.values.insert(dest.to_string(), value.map(str::to_string));
    }

    pub fn was_explicit(&self, name: &str) -> bool {
        self.explicit.iter().any(|e| e == name)
    }
}

/// Usage error (message for stderr, exit code 2).
#[derive(Debug, Clone)]
pub struct UsageError(pub String);

fn find_opt<'a>(opts: &'a [Opt], name: &str) -> Result<&'a Opt, String> {
    if let Some(o) = opts.iter().find(|o| o.names.contains(&name)) {
        return Ok(o);
    }
    if name.starts_with("--") {
        let matches: Vec<&Opt> = opts
            .iter()
            .filter(|o| o.names.iter().any(|n| n.starts_with(name)))
            .collect();
        let mut dedup: Vec<&str> = Vec::new();
        for o in &matches {
            for n in o.names {
                if n.starts_with(name) && !dedup.contains(n) {
                    dedup.push(n);
                }
            }
        }
        if matches.len() == 1 {
            return Ok(matches[0]);
        }
        if matches.len() > 1 {
            return Err(format!(
                "ambiguous option: {name} could match {}",
                dedup.join(", ")
            ));
        }
    }
    Err(format!("unrecognized arguments: {name}"))
}

/// Parse `args` against `opts` with `npos` required positionals (names).
pub fn parse(
    prog: &str,
    opts: &[Opt],
    positional: &[&str],
    args: &[OsString],
) -> Result<Namespace, UsageError> {
    let usage = |msg: &str| UsageError(format!("usage: kct {prog} [options] {}\nkct {prog}: error: {msg}", positional.join(" ")));
    let mut ns = Namespace::default();
    for o in opts {
        ns.values
            .entry(o.dest.to_string())
            .or_insert(o.default.map(str::to_string));
    }
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let mut i = 0;
    let mut only_pos = false;
    while i < args.len() {
        let a = &args[i];
        if only_pos || !a.starts_with('-') || a == "-" || a.parse::<f64>().is_ok() {
            ns.positionals.push(a.clone());
            i += 1;
            continue;
        }
        if a == "--" {
            only_pos = true;
            i += 1;
            continue;
        }
        if a == "-h" || a == "--help" {
            let mut s = format!("usage: kct {prog} [-h] [options] {}\n\noptions:\n", positional.join(" "));
            for o in opts {
                s.push_str(&format!("  {}{}\n", o.names.join(", "), if o.value { format!(" {}", o.dest.to_uppercase()) } else { String::new() }));
            }
            return Err(UsageError(format!("HELP:{s}")));
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let o = find_opt(opts, &name).map_err(|m| usage(&m))?;
        ns.explicit.push(o.names.last().unwrap_or(&"").to_string());
        ns.explicit.push(name.clone());
        if o.value {
            let val = match inline {
                Some(v) => v,
                None => {
                    i += 1;
                    match args.get(i) {
                        Some(v) => v.clone(),
                        None => {
                            return Err(usage(&format!("argument {}: expected one argument", o.names.join("/"))))
                        }
                    }
                }
            };
            if !o.choices.is_empty() && !o.choices.contains(&val.as_str()) {
                return Err(usage(&format!(
                    "argument {}: invalid choice: '{}' (choose from {})",
                    o.names.join("/"),
                    val,
                    o.choices.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(", ")
                )));
            }
            ns.set(o.dest, Some(&val));
        } else {
            if inline.is_some() {
                return Err(usage(&format!("argument {}: ignored explicit argument", name)));
            }
            ns.set(o.dest, Some(o.store));
        }
        i += 1;
    }
    if ns.positionals.len() < positional.len() {
        let missing: Vec<&str> = positional[ns.positionals.len()..].to_vec();
        return Err(usage(&format!(
            "the following arguments are required: {}",
            missing.join(", ")
        )));
    }
    Ok(ns)
}

/// Run `parse`, printing usage/help and mapping to an exit code.
pub fn parse_or_exit(
    prog: &str,
    opts: &[Opt],
    positional: &[&str],
    args: &[OsString],
) -> Result<Namespace, i32> {
    match parse(prog, opts, positional, args) {
        Ok(ns) => Ok(ns),
        Err(UsageError(msg)) => {
            if let Some(help) = msg.strip_prefix("HELP:") {
                print!("{help}");
                Err(0)
            } else {
                eprintln!("{msg}");
                Err(2)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn defaults_and_prefix() {
        let ns = parse("route", ROUTE_OPTS, &["pcb"], &os(&["b.kicad_pcb", "--strat", "basic", "--no-auto-pour"])).unwrap();
        assert_eq!(ns.positionals, vec!["b.kicad_pcb"]);
        assert_eq!(ns.get("strategy"), Some("basic"));
        assert_eq!(ns.tri("auto_pour"), Some(false));
        assert_eq!(ns.tri("auto_layers"), None);
        assert_eq!(ns.f64("clearance"), Some(0.15));
        assert_eq!(ns.get("grid"), Some("auto"));
        assert!(ns.flag("routing_plan"));
    }

    #[test]
    fn errors() {
        assert!(parse("route", ROUTE_OPTS, &["pcb"], &os(&[])).is_err());
        assert!(parse("route", ROUTE_OPTS, &["pcb"], &os(&["x", "--layers", "3"])).is_err());
        assert!(parse("route", ROUTE_OPTS, &["pcb"], &os(&["x", "--bogus"])).is_err());
        // Ambiguous prefix.
        assert!(parse("route", ROUTE_OPTS, &["pcb"], &os(&["x", "--auto"])).is_err());
    }
}
