//! Orderable via-in-pad fabrication processes (port of
//! `kicad_tools.manufacturers.fabrication_process`, Issue #5009).

use crate::jobj;
use crate::pyjson::{py_repr_str, Json};
use crate::utils::pyfmt::g;

const ELIGIBILITY_TOLERANCE_MM: f64 = 1e-4;

#[derive(Debug, Clone, PartialEq)]
pub struct FabricationProcess {
    pub process_id: &'static str,
    pub name: &'static str,
    pub min_layer_count: i64,
    pub min_via_drill_mm: f64,
    pub max_via_drill_mm: f64,
    pub min_annular_ring_mm: f64,
    pub requires_filled_and_capped: bool,
    pub min_component_hole_distance_mm: f64,
    pub source: &'static str,
}

impl FabricationProcess {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "process_id" => self.process_id,
            "name" => self.name,
            "min_layer_count" => self.min_layer_count,
            "min_via_drill_mm" => self.min_via_drill_mm,
            "max_via_drill_mm" => self.max_via_drill_mm,
            "min_annular_ring_mm" => self.min_annular_ring_mm,
            "requires_filled_and_capped" => self.requires_filled_and_capped,
            "min_component_hole_distance_mm" => self.min_component_hole_distance_mm,
            "source" => self.source,
        }
    }

    /// Reasons a via fails this process (empty = eligible).
    pub fn eligibility_reasons(
        &self,
        layer_count: Option<i64>,
        drill_mm: f64,
        annular_ring_mm: Option<f64>,
        nearest_other_hole_distance_mm: Option<f64>,
    ) -> Vec<String> {
        let tol = ELIGIBILITY_TOLERANCE_MM;
        let mut reasons = Vec::new();
        if let Some(lc) = layer_count {
            if lc < self.min_layer_count {
                reasons.push(format!(
                    "board has {lc} copper layer(s); process {} requires >= {}",
                    py_repr_str(self.process_id),
                    self.min_layer_count
                ));
            }
        }
        if drill_mm < self.min_via_drill_mm - tol {
            reasons.push(format!(
                "via drill {drill_mm:.3}mm is below process minimum {:.3}mm",
                self.min_via_drill_mm
            ));
        } else if drill_mm > self.max_via_drill_mm + tol {
            reasons.push(format!(
                "via drill {drill_mm:.3}mm exceeds process maximum {:.3}mm",
                self.max_via_drill_mm
            ));
        }
        if let Some(ar) = annular_ring_mm {
            if ar < self.min_annular_ring_mm - tol {
                reasons.push(format!(
                    "via annular ring {ar:.3}mm is below process minimum {:.3}mm",
                    self.min_annular_ring_mm
                ));
            }
        }
        if let Some(d) = nearest_other_hole_distance_mm {
            if d < self.min_component_hole_distance_mm - tol {
                reasons.push(format!(
                    "via is {d:.3}mm from the nearest other component hole; process requires >= {:.3}mm",
                    self.min_component_hole_distance_mm
                ));
            }
        }
        reasons
    }

    pub fn ordering_instructions(&self) -> String {
        let fill_note = if self.requires_filled_and_capped {
            "epoxy-filled and copper-capped (plated-over) vias"
        } else {
            "standard via processing"
        };
        format!(
            "Order the {} process ({}): a >= {}-layer stackup with {fill_note}, via drills \
             between {} and {} mm, annular ring >= {} mm, and in-pad vias kept >= {} mm from \
             any other component's drilled hole. Do not substitute ordinary open or merely \
             tented vias. Source: {}",
            self.name,
            self.process_id,
            self.min_layer_count,
            g(self.min_via_drill_mm),
            g(self.max_via_drill_mm),
            g(self.min_annular_ring_mm),
            g(self.min_component_hole_distance_mm),
            self.source
        )
    }
}

pub const JLCPCB_TIER1_POFV_4L: FabricationProcess = FabricationProcess {
    process_id: "jlcpcb-tier1-pofv-4l",
    name: "JLCPCB Capability Plus -- Plated-Over Filled Via (POFV), 4+ layer",
    min_layer_count: 4,
    min_via_drill_mm: 0.2,
    max_via_drill_mm: 0.5,
    min_annular_ring_mm: 0.10,
    requires_filled_and_capped: true,
    min_component_hole_distance_mm: 0.5,
    source: "https://jlcpcb.com/news/free-via-in-pad-6-20-layer-pcbs-pofv",
};

pub const PCBWAY_VIA_IN_PAD: FabricationProcess = FabricationProcess {
    process_id: "pcbway-via-in-pad",
    name: "PCBWay Via-in-Pad (epoxy-filled & capped)",
    min_layer_count: 2,
    min_via_drill_mm: 0.15,
    max_via_drill_mm: 0.5,
    min_annular_ring_mm: 0.15,
    requires_filled_and_capped: true,
    min_component_hole_distance_mm: 0.5,
    source: "https://www.pcbway.com/capabilities.html",
};

pub const FABRICATION_PROCESSES: &[FabricationProcess] = &[JLCPCB_TIER1_POFV_4L, PCBWAY_VIA_IN_PAD];

pub fn get_fabrication_process(process_id: Option<&str>) -> Option<&'static FabricationProcess> {
    let id = process_id.filter(|s| !s.is_empty())?;
    FABRICATION_PROCESSES.iter().find(|p| p.process_id == id)
}

/// `describe_selection(design_rules)`.
pub fn describe_selection(rules: &super::DesignRules) -> Option<Json> {
    let p = get_fabrication_process(rules.via_in_pad_process_id.as_deref())?;
    let mut d = p.to_dict();
    d.set("ordering_instructions", p.ordering_instructions());
    Some(d)
}
