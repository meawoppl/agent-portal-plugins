//! Branch-scoped current-path ampacity (port of
//! `kicad_tools.validate.rules.path_ampacity`, issue #4980).

use crate::manufacturers::DesignRules;
use crate::physics::ampacity::{adiabatic_fusing_current, width_for_current};
use crate::pyjson::{py_float_repr, py_repr_str, py_round};
use crate::router::current_paths::{
    audit_current_paths, CurrentPathSpec, PathResolution, ThermalDesignCurrent, UnmodeledCopper,
    STATUS_AMBIGUOUS, STATUS_UNRESOLVED,
};
use crate::schema::pcb::{Pcb, Segment};
use crate::utils::pyfmt::format_g;
use crate::validate::violations::{DRCResults, DRCViolation};

const EXTERNAL_LAYERS: [&str; 2] = ["F.Cu", "B.Cu"];
const RULE_ID: &str = "path_ampacity";

/// Upstream `PathAmpacityRule`.
#[derive(Debug, Clone, Default)]
pub struct PathAmpacityRule {
    pub specs: Vec<CurrentPathSpec>,
}

fn g4(x: f64) -> String {
    format_g(x, 4)
}

fn midpoint(s: &Segment) -> (f64, f64) {
    (
        py_round((s.start.0 + s.end.0) / 2.0, 3),
        py_round((s.start.1 + s.end.1) / 2.0, 3),
    )
}

fn oz(rules: &DesignRules, external: bool) -> f64 {
    if external {
        rules.outer_copper_oz
    } else {
        rules.inner_copper_oz
    }
}

impl PathAmpacityRule {
    pub fn new(specs: Vec<CurrentPathSpec>) -> Self {
        PathAmpacityRule { specs }
    }

    fn required_width_mm(current_a: f64, rules: &DesignRules, external: bool) -> f64 {
        let layer = if external { "external" } else { "internal" };
        width_for_current(current_a, oz(rules, external), 10.0, layer).unwrap_or(f64::NAN)
    }

    pub fn check(&self, pcb: &Pcb, design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        if self.specs.is_empty() {
            return results;
        }
        let audit = audit_current_paths(pcb, &self.specs);
        let segs = pcb.segments();
        for res in &audit.resolutions {
            if res.status == STATUS_UNRESOLVED || res.status == STATUS_AMBIGUOUS {
                results.add(unresolved_violation(res));
                continue;
            }
            let spec = &res.spec;
            let thermal = spec.thermal_design_current();
            if !thermal.assumption.is_empty() {
                results.add(assumption_notice(spec, &thermal));
            }
            if spec.pulsed_a.is_some() && spec.pulse_duration_s.is_none() {
                results.add(unchecked_fusing_violation(spec));
            }
            let mut cache: [Option<f64>; 2] = [None, None];
            for &si in &res.segments {
                let seg = &segs[si];
                let layer = seg.layer.as_str();
                let external = EXTERNAL_LAYERS.contains(&layer);
                let actual = seg.width;
                let slot = &mut cache[external as usize];
                let required = *slot.get_or_insert_with(|| {
                    Self::required_width_mm(thermal.current_a, design_rules, external)
                });
                if actual < required {
                    let layer_class = if external { "external" } else { "internal" };
                    let (x, y) = midpoint(seg);
                    results.add(
                        DRCViolation::new(
                            RULE_ID,
                            "error",
                            format!(
                                "Current path {} ({} -> {}) trace on {layer} too narrow for {}: \
                                 width {actual:.3}mm, requires {required:.3}mm (IPC-2221, {}oz \
                                 {layer_class})",
                                py_repr_str(&spec.name),
                                spec.source.label(),
                                spec.sink.label(),
                                thermal.description,
                                py_float_repr(oz(design_rules, external))
                            ),
                        )
                        .at(x, y)
                        .layer(layer)
                        .actual(actual)
                        .required(required)
                        .items([spec.name.clone(), spec.net_name.clone()]),
                    );
                }
                if let Some(v) = fusing_violation(spec, seg, external, design_rules) {
                    results.add(v);
                }
            }
        }
        for (net, list) in &audit.uncovered {
            for &si in list {
                results.add(uncovered_violation(net, &segs[si]));
            }
        }
        for (net, items) in &audit.unmodeled {
            for item in items {
                results.add(unmodeled_violation(net, item));
            }
        }
        results
    }
}

fn unresolved_violation(res: &PathResolution) -> DRCViolation {
    let spec = &res.spec;
    DRCViolation::new(
        RULE_ID,
        "error",
        format!(
            "Current path {} on net {} ({} -> {}) is {}: {}",
            py_repr_str(&spec.name),
            py_repr_str(&spec.net_name),
            spec.source.label(),
            spec.sink.label(),
            res.status,
            res.reason
        ),
    )
    .layer("")
    .items([spec.name.clone(), spec.net_name.clone()])
}

fn fusing_violation(
    spec: &CurrentPathSpec,
    seg: &Segment,
    external: bool,
    rules: &DesignRules,
) -> Option<DRCViolation> {
    let (pulsed, duration) = (spec.pulsed_a?, spec.pulse_duration_s?);
    let actual = seg.width;
    if actual <= 0.0 {
        return None;
    }
    let cu = oz(rules, external);
    let fusing = adiabatic_fusing_current(actual, cu, duration, 25.0).ok()?;
    if pulsed < fusing {
        return None;
    }
    let (x, y) = midpoint(seg);
    Some(
        DRCViolation::new(
            RULE_ID,
            "error",
            format!(
                "Current path {} ({} -> {}) trace on {} would fuse: declared {}A pulse for {}s \
                 meets or exceeds the {}A adiabatic fusing current of {actual:.3}mm of {}oz \
                 copper (Onderdonk)",
                py_repr_str(&spec.name),
                spec.source.label(),
                spec.sink.label(),
                seg.layer,
                g4(pulsed),
                g4(duration),
                g4(fusing),
                py_float_repr(cu)
            ),
        )
        .at(x, y)
        .layer(seg.layer.clone())
        .actual(pulsed)
        .required(fusing)
        .items([spec.name.clone(), spec.net_name.clone()]),
    )
}

fn unchecked_fusing_violation(spec: &CurrentPathSpec) -> DRCViolation {
    DRCViolation::new(
        RULE_ID,
        "warning",
        format!(
            "Current path {} on net {} declares a {}A pulse but no 'pulse_duration_s' -- \
             adiabatic fusing survivability was NOT checked for this branch",
            py_repr_str(&spec.name),
            py_repr_str(&spec.net_name),
            g4(spec.pulsed_a.unwrap_or(0.0))
        ),
    )
    .layer("")
    .items([spec.name.clone(), spec.net_name.clone()])
}

fn assumption_notice(spec: &CurrentPathSpec, thermal: &ThermalDesignCurrent) -> DRCViolation {
    DRCViolation::new(
        RULE_ID,
        "info",
        format!(
            "Current path {} on net {} sized at {}: {}",
            py_repr_str(&spec.name),
            py_repr_str(&spec.net_name),
            thermal.description,
            thermal.assumption
        ),
    )
    .layer("")
    .items([spec.name.clone(), spec.net_name.clone()])
}

fn uncovered_violation(net: &str, seg: &Segment) -> DRCViolation {
    let (x, y) = midpoint(seg);
    DRCViolation::new(
        RULE_ID,
        "warning",
        format!(
            "Net {} has declared current path(s) but this segment on {} is not covered by any \
             resolved path -- unmodeled/unverified current-carrying copper",
            py_repr_str(net),
            seg.layer
        ),
    )
    .at(x, y)
    .layer(seg.layer.clone())
    .items([net.to_string()])
}

fn unmodeled_violation(net: &str, item: &UnmodeledCopper) -> DRCViolation {
    DRCViolation::new(
        RULE_ID,
        "warning",
        format!(
            "Net {} has declared current path(s) but carries a same-net {} on {} that is not \
             modeled by the declared-path graph -- it may form a parallel return path around a \
             declared branch",
            py_repr_str(net),
            item.kind,
            item.layer
        ),
    )
    .at(item.location.0, item.location.1)
    .layer(item.layer.clone())
    .items([net.to_string()])
}
