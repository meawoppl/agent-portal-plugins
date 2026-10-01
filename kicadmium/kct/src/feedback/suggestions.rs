//! Fix suggestions for common DRC/ERC errors (port of
//! `kicad_tools.feedback.suggestions`).

use crate::drc::violation::{DRCViolation, ViolationType};
use crate::erc::violation::{ERCViolation, ERCViolationType};

/// Python truthiness of an optional float (`None` and `0.0` are falsy).
fn truthy(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x != 0.0)
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// A violation the generator understands.
pub enum AnyViolation<'a> {
    Drc(&'a DRCViolation),
    Erc(&'a ERCViolation),
}

/// Generate fix suggestions for DRC/ERC violations, most effective first.
#[derive(Debug, Default, Clone, Copy)]
pub struct FixSuggestionGenerator;

impl FixSuggestionGenerator {
    pub fn suggest(&self, violation: AnyViolation<'_>) -> Vec<String> {
        match violation {
            AnyViolation::Drc(v) => self.suggest_for_drc(v),
            AnyViolation::Erc(v) => self.suggest_for_erc(v),
        }
    }

    pub fn suggest_for_drc(&self, v: &DRCViolation) -> Vec<String> {
        use ViolationType::*;
        match v.vtype {
            CLEARANCE => suggest_clearance(v),
            COPPER_EDGE_CLEARANCE => suggest_edge_clearance(v),
            COURTYARD_OVERLAP => suggest_courtyard(v),
            UNCONNECTED_ITEMS => suggest_unconnected(v),
            SHORTING_ITEMS => suggest_shorting(v),
            TRACK_WIDTH => suggest_track_width(v),
            VIA_ANNULAR_WIDTH => suggest_annular_ring(v),
            VIA_HOLE_LARGER_THAN_PAD => strings(&[
                "Increase via pad diameter to accommodate the drill size",
                "Use a smaller drill size for the via",
                "Review via definitions in Board Setup > Design Rules",
                "Check if blind/buried via settings are correct",
            ]),
            DRILL_HOLE_TOO_SMALL => suggest_drill_hole(v),
            SILK_OVER_COPPER => strings(&[
                "Move silkscreen text/graphics away from exposed copper",
                "Shrink silkscreen text size to fit within available space",
                "Delete unnecessary silkscreen elements over pads",
                "Check footprint silkscreen layer in footprint editor",
                "Enable 'Clip silkscreen' option in Board Setup if available",
            ]),
            SILK_OVERLAP => strings(&[
                "Move overlapping silkscreen elements apart",
                "Reduce silkscreen text size",
                "Remove redundant silkscreen graphics",
                "Adjust reference designator positions in footprints",
                "Use smaller font for component values if needed",
            ]),
            SOLDER_MASK_BRIDGE => suggest_solder_mask(v),
            MISSING_FOOTPRINT => strings(&[
                "Assign footprint to symbol in schematic (press 'E' to edit)",
                "Check library path configuration in Preferences > Manage Libraries",
                "Verify footprint name matches library entry",
                "Update PCB from schematic to sync footprint assignments",
                "Create missing footprint in Footprint Editor if needed",
            ]),
            DUPLICATE_FOOTPRINT => strings(&[
                "Delete the duplicate footprint instance",
                "Update PCB from schematic to resolve duplicates",
                "Check for accidental copy-paste in PCB layout",
                "Verify component references are unique in schematic",
            ]),
            EXTRA_FOOTPRINT => strings(&[
                "Delete the extra footprint not present in schematic",
                "If intentional, add corresponding symbol to schematic",
                "Update PCB from schematic to sync component list",
                "Check for PCB-only components that need annotation",
            ]),
            MALFORMED_OUTLINE => strings(&[
                "Ensure board outline on Edge.Cuts layer forms a closed polygon",
                "Check for gaps or overlaps in board outline segments",
                "Use 'Board Setup > Board Outline' tools to validate",
                "Verify arc segments connect properly to line segments",
                "Remove duplicate or stacked outline segments",
            ]),
            HOLE_NEAR_HOLE => suggest_hole_near_hole(v),
            _ => suggest_generic_drc(v),
        }
    }

    pub fn suggest_for_erc(&self, v: &ERCViolation) -> Vec<String> {
        use ERCViolationType::*;
        match v.vtype {
            PIN_NOT_CONNECTED => {
                let mut s = Vec::new();
                if let Some(item) = v.items.iter().find(|i| i.to_lowercase().contains("pin")) {
                    s.push(format!("Add wire to connect {item}"));
                }
                s.extend(strings(&[
                    "Connect the pin to the appropriate net",
                    "Add a No-Connect (X) flag if the pin is intentionally unconnected",
                    "Check if the symbol pin configuration matches the datasheet",
                    "Verify the wire endpoint connects to the pin",
                ]));
                s
            }
            PIN_NOT_DRIVEN => strings(&[
                "Connect an output or bidirectional pin to drive this input",
                "Add a pull-up or pull-down resistor if floating is acceptable",
                "Connect to a power symbol if this is a power input",
                "Check symbol pin electrical type configuration",
                "Add explicit driver source (buffer, logic gate output, etc.)",
            ]),
            POWER_PIN_NOT_DRIVEN => strings(&[
                "Connect a power symbol (VCC, GND, +3V3, etc.) to drive the power pin",
                "Add a power flag symbol if power is supplied from the PCB",
                "Check that power symbol net names match component power pins",
                "Verify power pin electrical type in symbol editor",
                "Add PWR_FLAG symbol to indicate external power source",
            ]),
            NO_CONNECT_CONNECTED => strings(&[
                "Remove the wire connected to the no-connect pin",
                "Remove the no-connect flag if connection is intentional",
                "Check if the symbol pin should be a different type",
                "Verify schematic intent for this connection",
            ]),
            NO_CONNECT_DANGLING => strings(&[
                "Move the no-connect flag to connect directly to the unconnected pin",
                "Delete the no-connect flag if not needed",
                "Ensure the no-connect flag is on the pin endpoint",
                "Check for overlapping wires that may cause misalignment",
            ]),
            DUPLICATE_REFERENCE => {
                let mut s = v.suggestions.clone();
                s.extend(strings(&[
                    "Run 'Annotate Schematic' to reassign unique references",
                    "Manually edit one component's reference to be unique",
                    "Check for copy-paste errors that duplicated components",
                    "Verify multi-unit symbols have correct unit assignments",
                ]));
                if v.description.to_lowercase().contains("sheets") {
                    s.insert(
                        0,
                        "Check all hierarchical sheets for components sharing this reference"
                            .into(),
                    );
                }
                s
            }
            LABEL_DANGLING => strings(&[
                "Connect a wire to the label",
                "Delete the unused label",
                "Move the label to connect to a wire endpoint",
                "Check for invisible wire or junction at the label position",
            ]),
            GLOBAL_LABEL_DANGLING => strings(&[
                "Connect a wire to the global label",
                "Verify the global label is used on at least one other sheet",
                "Delete the global label if it's not needed",
                "Check global label spelling matches other sheets",
            ]),
            HIER_LABEL_MISMATCH => strings(&[
                "Ensure hierarchical labels match corresponding sheet pins",
                "Check spelling and case of hierarchical label names",
                "Add missing hierarchical labels or sheet pins",
                "Delete orphaned hierarchical labels/pins",
                "Verify hierarchical sheet symbol connections",
            ]),
            WIRE_DANGLING | UNCONNECTED_WIRE_ENDPOINT => strings(&[
                "Extend the wire to connect to a pin or junction",
                "Delete the dangling wire segment",
                "Add a no-connect flag if intentionally unconnected",
                "Check for nearly-connected wire endpoints (snap to grid)",
                "Merge the wire with an adjacent wire segment",
            ]),
            MISSING_UNIT => strings(&[
                "Add the missing unit of the multi-unit symbol",
                "Check if all units are required for your design",
                "Use 'Add Symbol' to add remaining units (same component)",
                "Verify symbol definition includes all required units",
            ]),
            UNANNOTATED => strings(&[
                "Run 'Annotate Schematic' from Tools menu",
                "Manually enter a reference designator for the symbol",
                "Check annotation settings (start number, prefix)",
                "Verify the symbol has a Reference field",
            ]),
            SIMILAR_LABELS => strings(&[
                "Verify label names are intentionally different (not a typo)",
                "Rename one label to match if they should be the same net",
                "Add distinguishing characters if labels should differ",
                "Check for case sensitivity issues in label names",
            ]),
            ENDPOINT_OFF_GRID => strings(&[
                "Move the wire endpoint to snap to the grid",
                "Use 'Edit > Cleanup Graphics' to fix off-grid issues",
                "Check symbol pin positions for off-grid placement",
                "Adjust grid settings if using non-standard grid",
            ]),
            MULTIPLE_NET_NAMES => strings(&[
                "Remove duplicate labels leaving only one net name",
                "Add a net tie symbol if nets should be connected",
                "Check for overlapping labels at the same position",
                "Verify hierarchical connections don't create conflicts",
            ]),
            LIB_SYMBOL_MISMATCH => strings(&[
                "Run 'kicad-cli sch update-from-library' to sync symbols with library definitions",
                "Open symbol in schematic editor and use 'Update Symbol from Library'",
                "Review symbol fields to check for intentional overrides before updating",
                "If overrides are intentional, exclude this violation in ERC settings",
            ]),
            FOOTPRINT_LINK_ISSUES => strings(&[
                "Reassign the footprint to one matching the symbol's footprint filters",
                "Run 'kicad-cli sch update-from-library' to reset footprint assignments",
                "Open symbol properties and verify the Footprint field matches an available footprint",
                "Check footprint filters in the symbol library editor",
            ]),
            PIN_TO_PIN => strings(&[
                "Check for output pins driving the same net (output-to-output conflict)",
                "Change one of the conflicting pin types if the connection is intentional",
                "Add a buffer or bus driver between conflicting outputs",
                "Review symbol pin electrical types in the symbol editor",
            ]),
            ISOLATED_PIN_LABEL => strings(&[
                "Connect additional pins or wires to the label",
                "Remove the label if it is not needed",
                "Check that the label is placed on a wire endpoint connected to a pin",
                "This is informational -- a single-pin label may be intentional for test points",
            ]),
            SINGLE_GLOBAL_LABEL => strings(&[
                "Add a matching global label on another sheet to complete the connection",
                "Remove the global label if inter-sheet connectivity is not needed",
                "Convert to a local label if the net is only used on one sheet",
                "This is informational -- a single global label may be intentional for future use",
            ]),
            _ => {
                let mut s = vec![
                    format!("Review '{}' violation in schematic", v.type_str),
                    "Check ERC settings in Schematic Setup > Electrical Rules".to_string(),
                ];
                if !v.sheet.is_empty() {
                    s.push(format!("Navigate to sheet '{}' to inspect the issue", v.sheet));
                }
                if v.pos_x != 0.0 || v.pos_y != 0.0 {
                    s.push(format!(
                        "Inspect area around ({:.1}, {:.1})",
                        v.pos_x, v.pos_y
                    ));
                }
                s
            }
        }
    }
}

fn suggest_clearance(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(req), Some(act)) = (truthy(v.required_value_mm), truthy(v.actual_value_mm)) {
        let gap = req - act;
        s.push(format!(
            "Move affected elements apart by at least {gap:.2}mm to meet clearance requirement of {req:.2}mm"
        ));
    }
    let lower: Vec<String> = v.items.iter().map(|i| i.to_lowercase()).collect();
    let involves_track = lower
        .iter()
        .any(|i| i.contains("track") || i.contains("segment"));
    let involves_via = lower.iter().any(|i| i.contains("via"));
    let involves_pad = lower.iter().any(|i| i.contains("pad"));
    if involves_track {
        if v.nets.is_empty() {
            s.push("Reroute the track to increase spacing".into());
        } else {
            let net_name = if v.nets.len() == 1 {
                v.nets[0].clone()
            } else {
                v.nets.join("/")
            };
            s.push(format!("Reroute net '{net_name}' around the obstruction"));
        }
    }
    if involves_via {
        s.push("Move via to a different location or change to a smaller via size".into());
        s.push("Consider using a blind or buried via if design permits".into());
    }
    if involves_pad {
        s.push("Adjust component placement to increase pad-to-pad spacing".into());
        s.push("Check if a smaller footprint variant is available".into());
    }
    if v.nets.len() == 2 {
        s.push(format!(
            "Review net class rules for '{}' and '{}'",
            v.nets[0], v.nets[1]
        ));
    }
    if s.is_empty() {
        s.push("Increase spacing between copper elements".into());
        s.push("Check design rules in Board Setup > Design Rules".into());
    }
    s
}

fn suggest_edge_clearance(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(act), Some(req)) = (truthy(v.actual_value_mm), truthy(v.required_value_mm)) {
        s.push(format!(
            "Move copper {:.2}mm inward from board edge (required: {req:.2}mm)",
            req - act
        ));
    }
    s.extend(strings(&[
        "Move affected tracks/pads away from the board edge",
        "Extend the board outline if the current size is not critical",
        "Check Edge.Cuts layer for correct board boundary definition",
        "Verify manufacturer edge clearance requirements (typically 0.25-0.5mm)",
    ]));
    s
}

fn suggest_courtyard(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    let components: Vec<&String> = v
        .items
        .iter()
        .filter(|i| i.chars().any(char::is_alphabetic))
        .collect();
    if components.len() >= 2 {
        s.push(format!(
            "Move {} or {} to eliminate overlap",
            components[0], components[1]
        ));
    }
    s.extend(strings(&[
        "Increase spacing between components to clear courtyards",
        "Check if components can be rotated to reduce overlap area",
        "Consider using tighter courtyard footprint variants if available",
        "Verify the courtyard outline is correctly defined in footprint editor",
    ]));
    s
}

fn suggest_unconnected(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let Some(net) = v.nets.first() {
        s.push(format!("Add wire/trace to connect to net '{net}'"));
        s.push(format!("Route the incomplete connection for net '{net}'"));
    }
    if v.items.iter().any(|i| i.to_lowercase().contains("pad")) {
        s.push("Verify pad is correctly assigned in schematic".into());
        s.push("Check symbol-to-footprint pin mapping".into());
    }
    if v.items.iter().any(|i| i.to_lowercase().contains("zone")) {
        s.push("Refill copper zones to regenerate connections".into());
        s.push("Check zone priority and clearance settings".into());
    }
    s.extend(strings(&[
        "Run the autorouter for incomplete connections",
        "Verify netlist is up-to-date (update PCB from schematic)",
        "Check for broken or missing net ties",
    ]));
    s
}

fn suggest_shorting(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if v.nets.len() >= 2 {
        let (n1, n2) = (&v.nets[0], &v.nets[1]);
        s.push(format!("Remove the connection between '{n1}' and '{n2}'"));
        s.push(format!(
            "Reroute one of the traces for '{n1}' or '{n2}' to eliminate overlap"
        ));
    }
    s.extend(strings(&[
        "Delete the offending trace segment or via causing the short",
        "Move the conflicting copper elements to different layers",
        "Check for accidental copper pour connections",
        "Verify zone settings and net assignments",
    ]));
    s
}

fn suggest_track_width(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(req), Some(act)) = (truthy(v.required_value_mm), truthy(v.actual_value_mm)) {
        s.push(format!(
            "Widen track from {act:.3}mm to at least {req:.3}mm"
        ));
    }
    if let Some(net) = v.nets.first() {
        s.push(format!(
            "Update net class for '{net}' to set proper track width"
        ));
    }
    s.extend(strings(&[
        "Check design rules in Board Setup > Design Rules > Net Classes",
        "Edit track properties to increase width (select and press 'E')",
        "Consider using a different net class for power/signal nets",
        "Verify manufacturer minimum track width capability",
    ]));
    s
}

fn suggest_annular_ring(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(act), Some(req)) = (truthy(v.actual_value_mm), truthy(v.required_value_mm)) {
        s.push(format!(
            "Increase via pad size by {:.3}mm to meet annular ring requirement",
            (req - act) * 2.0
        ));
    }
    s.extend(strings(&[
        "Use a larger via pad size in Board Setup > Design Rules > Via",
        "Use a smaller drill size while keeping the same pad size",
        "Check manufacturer minimum annular ring requirement (typically 0.1-0.15mm)",
        "Switch to a different via size preset that meets requirements",
    ]));
    s
}

fn suggest_drill_hole(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(act), Some(req)) = (truthy(v.actual_value_mm), truthy(v.required_value_mm)) {
        s.push(format!(
            "Increase drill size from {act:.3}mm to at least {req:.3}mm"
        ));
    }
    s.extend(strings(&[
        "Check manufacturer minimum drill size capability",
        "Update via or pad drill settings in Board Setup",
        "Consider using a different manufacturer with smaller drill capability",
        "For vias, use laser-drilled microvias if available",
    ]));
    s
}

fn suggest_solder_mask(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(_), Some(req)) = (truthy(v.actual_value_mm), truthy(v.required_value_mm)) {
        s.push(format!(
            "Increase solder mask expansion or spacing to at least {req:.3}mm"
        ));
    }
    s.extend(strings(&[
        "Adjust solder mask clearance in Board Setup > Board Stackup",
        "Reduce pad sizes if electrically acceptable",
        "Increase spacing between pads/tracks",
        "Check manufacturer solder mask bridge capability (typically 0.1mm)",
    ]));
    s
}

fn suggest_hole_near_hole(v: &DRCViolation) -> Vec<String> {
    let mut s = Vec::new();
    if let (Some(act), Some(req)) = (truthy(v.actual_value_mm), truthy(v.required_value_mm)) {
        s.push(format!(
            "Move holes apart by at least {:.2}mm to meet spacing requirement",
            req - act
        ));
    }
    s.extend(strings(&[
        "Relocate vias or mounting holes to increase spacing",
        "Use smaller drill sizes if design permits",
        "Check manufacturer hole-to-hole spacing requirements",
        "Consider using slot instead of multiple close holes",
    ]));
    s
}

fn suggest_generic_drc(v: &DRCViolation) -> Vec<String> {
    let mut s = vec![
        format!(
            "Review '{}' violation details and affected elements",
            v.type_str
        ),
        "Check design rules in Board Setup > Design Rules".to_string(),
        "Consult manufacturer DFM guidelines for this violation type".to_string(),
    ];
    if !v.nets.is_empty() {
        s.push(format!(
            "Check net class settings for affected nets: {}",
            v.nets.join(", ")
        ));
    }
    if let Some(loc) = v.locations.first() {
        s.push(format!(
            "Inspect area around ({:.2}, {:.2})mm",
            loc.x_mm, loc.y_mm
        ));
    }
    s
}

/// Convenience: suggestions for a DRC violation.
pub fn generate_drc_suggestions(violation: &DRCViolation) -> Vec<String> {
    FixSuggestionGenerator.suggest_for_drc(violation)
}

/// Convenience: suggestions for an ERC violation.
pub fn generate_erc_suggestions(violation: &ERCViolation) -> Vec<String> {
    FixSuggestionGenerator.suggest_for_erc(violation)
}
