//! Pure-Rust DRC checker (port of `kicad_tools.validate.checker`).
//!
//! [`DRCChecker`] runs every rule family against a loaded [`Pcb`] and the
//! manufacturer's [`DesignRules`] without kicad-cli.

use std::collections::HashSet;

use anyhow::Result;

use crate::manufacturers::{self, DesignRules};
use crate::pyjson::py_round;
use crate::router::current_paths::CurrentPathSpec;
use crate::router::rules::NetClassMap;
use crate::schema::pcb::Pcb;
use crate::validate::mask_copper::MaskCopperRequest;
use crate::validate::rules;
use crate::validate::rules::courtyard_waivers::CourtyardWaivers;
use crate::validate::violations::{DRCResults, DRCViolation};

/// Reporting bucket names (Issue #3803).
pub const CATEGORY_MANUFACTURING: &str = "manufacturing";
pub const CATEGORY_ADVISORY: &str = "advisory-quality";

/// Ordered `check_*` names `check_all` invokes (upstream `CHECK_ALL_METHODS`).
pub const CHECK_ALL_METHODS: &[&str] = &[
    "check_ampacity",
    "check_path_ampacity",
    "check_clearances",
    "check_physical_copper_gap",
    "check_connectivity",
    "check_connector_access",
    "check_segment_zone_clearances",
    "check_via_zone_clearances",
    "check_copper_slivers",
    "check_courtyard_overlap",
    "check_dangling_copper",
    "check_diffpair_clearance_intra",
    "check_diffpair_length_skew",
    "check_diffpair_routing_continuity",
    "check_dimensions",
    "check_edge_clearances",
    "check_impedance",
    "check_isolated_copper",
    "check_match_group_length_skew",
    "check_silkscreen",
    "check_solder_mask_pads",
    "check_mask_to_copper",
    "check_footprint_placement",
    "check_netlist",
    "check_pin1_markers",
    "check_single_pad_nets",
    "check_pad_grid_alignment",
    "check_via_in_pad",
    "check_via_under_body",
    "check_width_consistency",
    "check_zero_length_segments",
    "check_zones",
];

/// Gating-advisory rule ids (upstream `ADVISORY_RULE_IDS`).
pub const ADVISORY_RULE_IDS: &[&str] = &["connectivity"];

/// `rule_id -> reporting category` (upstream `RULE_CATEGORY`).
pub const RULE_CATEGORY: &[(&str, &str)] = &[
    ("connectivity", CATEGORY_ADVISORY),
    ("ampacity", CATEGORY_ADVISORY),
    ("path_ampacity", CATEGORY_ADVISORY),
    ("connector_edge_access", CATEGORY_ADVISORY),
    ("connector_edge_distance", CATEGORY_ADVISORY),
    ("via_under_body", CATEGORY_ADVISORY),
    ("copper_sliver", CATEGORY_ADVISORY),
    ("width_consistency", CATEGORY_ADVISORY),
    ("width_island", CATEGORY_ADVISORY),
    ("width_transition", CATEGORY_ADVISORY),
    ("dangling_copper", CATEGORY_ADVISORY),
    ("track_dangling", CATEGORY_ADVISORY),
    ("via_dangling", CATEGORY_ADVISORY),
    ("isolated_copper", CATEGORY_ADVISORY),
    ("sch_field_offset", CATEGORY_ADVISORY),
    ("sch_field_overlap", CATEGORY_ADVISORY),
    ("doc_drift_stale_pin", CATEGORY_ADVISORY),
    ("doc_drift_unresolvable_pin", CATEGORY_ADVISORY),
    ("impedance", CATEGORY_ADVISORY),
    ("diffpair_clearance_intra", CATEGORY_ADVISORY),
    ("diffpair_length_skew", CATEGORY_ADVISORY),
    ("diffpair_routing_continuity", CATEGORY_ADVISORY),
    ("match_group_length_skew", CATEGORY_ADVISORY),
    ("pin1_marker_missing", CATEGORY_ADVISORY),
    ("pin1_marker_obscured", CATEGORY_ADVISORY),
    ("silk_over_copper", CATEGORY_ADVISORY),
    ("silk_edge_clearance", CATEGORY_ADVISORY),
    ("silk_geometry_unmodeled", CATEGORY_ADVISORY),
    ("silkscreen_line_width", CATEGORY_ADVISORY),
    ("silkscreen_over_pad", CATEGORY_ADVISORY),
    ("silkscreen_text_height", CATEGORY_ADVISORY),
    ("clearance", CATEGORY_MANUFACTURING),
    ("clearance_net0_bridge", CATEGORY_MANUFACTURING),
    ("clearance_pad_zone", CATEGORY_MANUFACTURING),
    ("clearance_segment_zone", CATEGORY_MANUFACTURING),
    ("clearance_via_zone", CATEGORY_MANUFACTURING),
    ("edge_clearance", CATEGORY_MANUFACTURING),
    ("edge_clearance_pad", CATEGORY_MANUFACTURING),
    ("edge_clearance_pad_hole", CATEGORY_MANUFACTURING),
    ("edge_clearance_trace", CATEGORY_MANUFACTURING),
    ("edge_clearance_via", CATEGORY_MANUFACTURING),
    ("edge_clearance_zone", CATEGORY_MANUFACTURING),
    ("hole_to_hole_clearance", CATEGORY_MANUFACTURING),
    ("solder_mask_clearance", CATEGORY_MANUFACTURING),
    ("solder_mask_pad", CATEGORY_MANUFACTURING),
    ("via_in_pad", CATEGORY_MANUFACTURING),
    ("via_in_pad_process_missing", CATEGORY_MANUFACTURING),
    ("via_in_pad_process_ineligible", CATEGORY_MANUFACTURING),
    ("min_pad_size", CATEGORY_MANUFACTURING),
    ("pth_annular_ring", CATEGORY_MANUFACTURING),
    ("pad_grid", CATEGORY_MANUFACTURING),
    ("dimensions", CATEGORY_MANUFACTURING),
    ("dimension_trace_width", CATEGORY_MANUFACTURING),
    ("dimension_via_diameter", CATEGORY_MANUFACTURING),
    ("dimension_via_drill", CATEGORY_MANUFACTURING),
    ("dimension_annular_ring", CATEGORY_MANUFACTURING),
    ("footprint_outside_board", CATEGORY_MANUFACTURING),
    ("single_pad_net", CATEGORY_MANUFACTURING),
    ("net_undeclared", CATEGORY_MANUFACTURING),
    ("zone_fill", CATEGORY_MANUFACTURING),
    ("zone_fill_disabled", CATEGORY_MANUFACTURING),
    ("zone_no_net", CATEGORY_MANUFACTURING),
    ("zone_unfilled", CATEGORY_MANUFACTURING),
];

const CATEGORY_PREFIXES: &[(&str, &str)] = &[
    ("diffpair", CATEGORY_ADVISORY),
    ("match_group", CATEGORY_ADVISORY),
    ("silk", CATEGORY_ADVISORY),
    ("edge_clearance", CATEGORY_MANUFACTURING),
    ("clearance", CATEGORY_MANUFACTURING),
    ("dimension", CATEGORY_MANUFACTURING),
    ("zone", CATEGORY_MANUFACTURING),
    ("via", CATEGORY_MANUFACTURING),
    ("hole", CATEGORY_MANUFACTURING),
    ("drill", CATEGORY_MANUFACTURING),
    ("solder_mask", CATEGORY_MANUFACTURING),
    ("pth_", CATEGORY_MANUFACTURING),
];

/// `DRCChecker.is_advisory_rule`.
pub fn is_advisory_rule(rule_id: &str) -> bool {
    ADVISORY_RULE_IDS.contains(&rule_id)
}

/// `DRCChecker.category_for_rule`.
pub fn category_for_rule(rule_id: &str) -> &'static str {
    if let Some((_, c)) = RULE_CATEGORY.iter().find(|(r, _)| *r == rule_id) {
        return c;
    }
    for (prefix, c) in CATEGORY_PREFIXES {
        if rule_id.starts_with(prefix) {
            return c;
        }
    }
    CATEGORY_MANUFACTURING
}

/// Constructor options (upstream keyword arguments).
#[derive(Debug, Clone)]
pub struct DRCCheckerOptions {
    pub manufacturer: String,
    pub layers: i64,
    pub copper_oz: f64,
    pub suppress_library: bool,
    pub net_class_map: Option<NetClassMap>,
    pub warn_on_inactive_skew_rules: bool,
    pub verbose: bool,
    pub emit_measurements: bool,
    pub courtyard_waivers: Option<CourtyardWaivers>,
    pub strict_connectivity: bool,
    pub copper_oz_outer: Option<f64>,
    pub copper_oz_inner: Option<f64>,
    pub current_path_specs: Vec<CurrentPathSpec>,
    pub mask_copper_request: Option<MaskCopperRequest>,
    pub physical_copper_gap_mm: Option<f64>,
}

impl Default for DRCCheckerOptions {
    fn default() -> Self {
        DRCCheckerOptions {
            manufacturer: "jlcpcb".into(),
            layers: 4,
            copper_oz: 1.0,
            suppress_library: false,
            net_class_map: None,
            warn_on_inactive_skew_rules: true,
            verbose: false,
            emit_measurements: false,
            courtyard_waivers: None,
            strict_connectivity: true,
            copper_oz_outer: None,
            copper_oz_inner: None,
            current_path_specs: Vec::new(),
            mask_copper_request: None,
            physical_copper_gap_mm: None,
        }
    }
}

/// Pure-Rust DRC checker (upstream `DRCChecker`).
pub struct DRCChecker<'a> {
    pub pcb: &'a Pcb,
    pub design_rules: DesignRules,
    pub manufacturer: String,
    pub layers: i64,
    pub copper_oz: f64,
    pub suppress_library: bool,
    pub net_class_map: Option<NetClassMap>,
    pub courtyard_waivers: Option<CourtyardWaivers>,
    pub current_path_specs: Vec<CurrentPathSpec>,
    pub strict_connectivity: bool,
    pub warn_on_inactive_skew_rules: bool,
    pub verbose: bool,
    pub physical_copper_gap_mm: Option<f64>,
    pub emit_measurements: bool,
    pub mask_copper_request: Option<MaskCopperRequest>,
    /// Opt-in width-consistency audit options (`None` keeps it out of
    /// `check_all`).
    pub width_consistency_options: Option<Vec<(String, f64)>>,
    emit_skew_info: bool,
    inactive_skew_warned: std::cell::RefCell<HashSet<String>>,
}

impl<'a> DRCChecker<'a> {
    /// `DRCChecker(pcb, ...)`; errors where upstream raises `ValueError`.
    pub fn new(pcb: &'a Pcb, opts: DRCCheckerOptions) -> Result<Self> {
        if let Some(gap) = opts.physical_copper_gap_mm {
            if !gap.is_finite() || gap <= 0.0 {
                anyhow::bail!("Physical copper gap must be finite and positive");
            }
        }
        let layers_key = u8::try_from(opts.layers).unwrap_or(0);
        let mut design_rules = get_design_rules(&opts.manufacturer, layers_key, opts.copper_oz)?;
        if let Some(outer) = opts.copper_oz_outer {
            design_rules.outer_copper_oz = outer;
        }
        if let Some(inner) = opts.copper_oz_inner {
            design_rules.inner_copper_oz = inner;
        }
        Ok(DRCChecker {
            pcb,
            design_rules,
            manufacturer: opts.manufacturer,
            layers: opts.layers,
            copper_oz: opts.copper_oz,
            suppress_library: opts.suppress_library,
            net_class_map: opts.net_class_map,
            courtyard_waivers: opts.courtyard_waivers,
            current_path_specs: opts.current_path_specs,
            strict_connectivity: opts.strict_connectivity,
            warn_on_inactive_skew_rules: opts.warn_on_inactive_skew_rules,
            verbose: opts.verbose,
            physical_copper_gap_mm: opts.physical_copper_gap_mm,
            emit_measurements: opts.emit_measurements,
            emit_skew_info: opts.verbose || opts.emit_measurements,
            mask_copper_request: opts.mask_copper_request,
            width_consistency_options: None,
            inactive_skew_warned: Default::default(),
        })
    }

    /// Whether skew/continuity rules emit their measured info findings.
    pub fn emit_skew_info(&self) -> bool {
        self.emit_skew_info
    }

    /// Shift board-relative violation locations back to sheet-absolute
    /// coordinates (upstream `_absolutize`, issue #4025).
    pub fn absolutize(&self, mut results: DRCResults) -> DRCResults {
        let (ox, oy) = self.pcb.board_origin();
        if ox == 0.0 && oy == 0.0 {
            return results;
        }
        for v in &mut results.violations {
            if let Some((lx, ly)) = v.location {
                v.location = Some((py_round(lx + ox, 3), py_round(ly + oy, 3)));
            }
        }
        results
    }

    /// Run every check in [`CHECK_ALL_METHODS`] order.
    pub fn check_all(&self, pad_grid_auto_derive: bool) -> DRCResults {
        let mut results = DRCResults::new();
        for name in CHECK_ALL_METHODS {
            if *name == "check_mask_to_copper" && self.mask_copper_request.is_none() {
                continue;
            }
            if *name == "check_width_consistency" && self.width_consistency_options.is_none() {
                continue;
            }
            let r = if *name == "check_pad_grid_alignment" {
                self.check_pad_grid_alignment(0.1, None, pad_grid_auto_derive, true)
            } else {
                self.run_named(name)
            };
            results.merge(r);
        }
        results
    }

    /// Dispatch a `check_*` method by upstream name.
    pub fn run_named(&self, name: &str) -> DRCResults {
        match name {
            "check_ampacity" => self.check_ampacity(),
            "check_path_ampacity" => self.check_path_ampacity(),
            "check_clearances" => self.check_clearances(),
            "check_physical_copper_gap" => self.check_physical_copper_gap(),
            "check_connectivity" => self.check_connectivity(),
            "check_connector_access" => self.check_connector_access(),
            "check_segment_zone_clearances" => self.check_segment_zone_clearances(),
            "check_via_zone_clearances" => self.check_via_zone_clearances(),
            "check_copper_slivers" => self.check_copper_slivers(),
            "check_courtyard_overlap" => self.check_courtyard_overlap(),
            "check_dangling_copper" => self.check_dangling_copper(),
            "check_diffpair_clearance_intra" => self.check_diffpair_clearance_intra(),
            "check_diffpair_length_skew" => self.check_diffpair_length_skew(),
            "check_diffpair_routing_continuity" => self.check_diffpair_routing_continuity(),
            "check_dimensions" => self.check_dimensions(),
            "check_edge_clearances" => self.check_edge_clearances(),
            "check_impedance" => self.check_impedance(),
            "check_isolated_copper" => self.check_isolated_copper(),
            "check_match_group_length_skew" => self.check_match_group_length_skew(),
            "check_silkscreen" => self.check_silkscreen(),
            "check_solder_mask_pads" => self.check_solder_mask_pads(),
            "check_mask_to_copper" => self.check_mask_to_copper(),
            "check_footprint_placement" => self.check_footprint_placement(),
            "check_netlist" => self.check_netlist(),
            "check_pin1_markers" => self.check_pin1_markers(),
            "check_single_pad_nets" => self.check_single_pad_nets(),
            "check_pad_grid_alignment" => self.check_pad_grid_alignment(0.1, None, false, true),
            "check_via_in_pad" => self.check_via_in_pad(),
            "check_via_under_body" => self.check_via_under_body(),
            "check_width_consistency" => self.check_width_consistency(),
            "check_zero_length_segments" => self.check_zero_length_segments(),
            "check_zones" => self.check_zones(),
            other => panic!("unknown DRCChecker method {other}"),
        }
    }

    pub fn check_physical_copper_gap(&self) -> DRCResults {
        match self.physical_copper_gap_mm {
            None => DRCResults::new(),
            Some(gap) => {
                rules::physical_gap::check_physical_copper_gap(self.pcb, &self.design_rules, gap)
            }
        }
    }

    pub fn check_clearances(&self) -> DRCResults {
        let rule = rules::clearance::ClearanceRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_segment_zone_clearances(&self) -> DRCResults {
        let rule = rules::clearance::SegmentZoneClearanceRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_via_zone_clearances(&self) -> DRCResults {
        let rule = rules::clearance::ViaZoneClearanceRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_copper_slivers(&self) -> DRCResults {
        let rule = rules::copper_sliver::CopperSliverRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_courtyard_overlap(&self) -> DRCResults {
        let rule = rules::courtyard::CourtyardOverlapRule::new(self.courtyard_waivers.clone());
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_connectivity(&self) -> DRCResults {
        let rule = rules::connectivity::ConnectivityRule::new(self.strict_connectivity);
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_dangling_copper(&self) -> DRCResults {
        let rule = rules::dangling_copper::DanglingCopperRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_diffpair_clearance_intra(&self) -> DRCResults {
        let rule = rules::diffpair_clearance_intra::DiffPairClearanceIntraRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    /// One-time stderr warning that a sidecar-gated skew rule is inactive
    /// (upstream `_warn_inactive_skew_rule`, issue #3917).
    pub fn warn_inactive_skew_rule(&self, rule_name: &str) {
        if !self.warn_on_inactive_skew_rules {
            return;
        }
        if !self
            .inactive_skew_warned
            .borrow_mut()
            .insert(rule_name.to_string())
        {
            return;
        }
        eprintln!(
            "WARNING: rule {} is INACTIVE without a net-class-map sidecar and will silently \
             pass; supply a net-class-map sidecar to validate length-match skew.",
            crate::pyjson::py_repr_str(rule_name)
        );
    }

    pub fn check_diffpair_length_skew(&self) -> DRCResults {
        if self.net_class_map.is_none() {
            self.warn_inactive_skew_rule("diffpair_length_skew");
        }
        let rule = rules::diffpair_length_skew::DiffPairLengthSkewRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_diffpair_routing_continuity(&self) -> DRCResults {
        if self.net_class_map.is_none() {
            self.warn_inactive_skew_rule("diffpair_routing_continuity");
        }
        let rule = rules::diffpair_routing_continuity::DiffPairRoutingContinuityRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_dimensions(&self) -> DRCResults {
        let rule = rules::dimensions::DimensionRules;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_edge_clearances(&self) -> DRCResults {
        let rule = rules::edge::EdgeClearanceRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_ampacity(&self) -> DRCResults {
        let specs =
            crate::validate::ampacity_specs::derive_ampacity_specs(self.net_class_map.as_ref());
        let rule = rules::ampacity::AmpacityRule::new(specs);
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_path_ampacity(&self) -> DRCResults {
        let rule = rules::path_ampacity::PathAmpacityRule::new(self.current_path_specs.clone());
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_impedance(&self) -> DRCResults {
        let rule = rules::impedance::ImpedanceRule::default();
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_match_group_length_skew(&self) -> DRCResults {
        if self.net_class_map.is_none() {
            self.warn_inactive_skew_rule("match_group_length_skew");
        }
        let rule = rules::match_group_length_skew::MatchGroupLengthSkewRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_silkscreen(&self) -> DRCResults {
        self.absolutize(rules::silkscreen::check_all_silkscreen(
            self.pcb,
            &self.design_rules,
            self.suppress_library,
        ))
    }

    pub fn check_pin1_markers(&self) -> DRCResults {
        let rule = rules::pin1_marker::Pin1MarkerRule::default();
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_mask_to_copper(&self) -> DRCResults {
        crate::validate::mask_copper::check_requested(self.pcb, self.mask_copper_request.as_ref())
    }

    pub fn check_solder_mask_pads(&self) -> DRCResults {
        let rule = rules::solder_mask::SolderMaskPadRules;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_footprint_placement(&self) -> DRCResults {
        let rule = rules::placement::FootprintOutsideBoardRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_connector_access(&self) -> DRCResults {
        let rule = rules::connector_access::ConnectorEdgeAccessRule::new(
            self.verbose || self.emit_measurements,
        );
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_netlist(&self) -> DRCResults {
        let rule = rules::netlist::NetlistRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_single_pad_nets(&self) -> DRCResults {
        let rule = rules::single_pad_net::SinglePadNetRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_via_in_pad(&self) -> DRCResults {
        let rule = rules::via_in_pad::ViaInPadRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_width_consistency(&self) -> DRCResults {
        let rule = rules::width_consistency::WidthConsistencyRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_via_under_body(&self) -> DRCResults {
        let rule = rules::via_under_body::ViaUnderBodyRule::default();
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_zero_length_segments(&self) -> DRCResults {
        let rule = rules::zero_length_segment::ZeroLengthSegmentRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_zones(&self) -> DRCResults {
        let rule = rules::zone_fill::ZoneFillRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    pub fn check_isolated_copper(&self) -> DRCResults {
        let rule = rules::zone_fill::IsolatedCopperRule;
        self.absolutize(rule.check(self.pcb, &self.design_rules))
    }

    /// Router-grid pad alignment (upstream `check_pad_grid_alignment`).
    /// Locations are already sheet-absolute (not absolutized).
    pub fn check_pad_grid_alignment(
        &self,
        grid_resolution: f64,
        threshold: Option<f64>,
        auto_derive_threshold: bool,
        aggregate: bool,
    ) -> DRCResults {
        use crate::router::preflight::check_pad_grid_alignment;
        let mut results = DRCResults::new();
        if self.pcb.path().is_none() {
            results.rules_checked += 1;
            return results;
        }
        let report = check_pad_grid_alignment(
            self.pcb.sexp(),
            grid_resolution,
            threshold,
            self.design_rules.min_clearance_mm,
            auto_derive_threshold,
        );
        results.rules_checked += 1;
        let per_pad = |results: &mut DRCResults, pads: &[&crate::router::preflight::PreflightOffGridPad]| {
            for pad in pads {
                let label = pad.label();
                let mut v = DRCViolation::new(
                    "pad_grid",
                    "warning",
                    pad.message(report.grid_resolution, report.suggested_grid),
                )
                .at(pad.x, pad.y)
                .actual(pad.offset_mm)
                .required(report.threshold);
                if !label.is_empty() {
                    v = v.items([label]);
                }
                results.add(v);
            }
        };
        if !aggregate {
            let all: Vec<_> = report.off_grid_pads.iter().collect();
            per_pad(&mut results, &all);
            return results;
        }
        let grid = crate::pyjson::py_float_repr(report.grid_resolution);
        for (reference, pads) in report.grouped_by_ref() {
            if pads.len() == 1 {
                per_pad(&mut results, &pads);
                continue;
            }
            // `max(pads, key=offset)`: first maximum.
            let mut example = pads[0];
            for p in &pads[1..] {
                if p.offset_mm > example.offset_mm {
                    example = p;
                }
            }
            let fp = if example.footprint_name.is_empty() {
                String::new()
            } else {
                format!(", footprint {}", example.footprint_name)
            };
            let head = format!(
                "{reference}: {} pads off-grid by up to {:.3}mm (grid {grid}mm{fp}).\n  Example: pad {} at ({:.3}, {:.3}).\n",
                pads.len(),
                example.offset_mm,
                example.label(),
                example.x,
                example.y
            );
            let tail = match report.suggested_grid {
                Some(sg) => format!(
                    "  Suggested fix: round pad positions OR set finer router grid ({}mm would align all pads).\n  Use --verbose for per-pad detail.",
                    crate::pyjson::py_float_repr(sg)
                ),
                None => format!(
                    "  Suggested fix: round pad positions to the router grid (footprint pitch may not align to {grid}mm).\n  Use --verbose for per-pad detail."
                ),
            };
            let mut v = DRCViolation::new("pad_grid", "warning", head + &tail)
                .at(example.x, example.y)
                .actual(example.offset_mm)
                .required(report.threshold);
            if !reference.is_empty() {
                v = v.items([reference.clone()]);
            }
            results.add(v);
        }
        results
    }
}

/// `get_profile(manufacturer).get_design_rules(layers, copper_oz)`.
pub fn get_design_rules(manufacturer: &str, layers: u8, copper_oz: f64) -> Result<DesignRules> {
    manufacturers::rules(manufacturer, layers, copper_oz)
}

/// Build a pad-grid style violation (shared helper for callers).
pub fn warning(rule_id: &str, message: String) -> DRCViolation {
    DRCViolation::new(rule_id, "warning", message)
}
