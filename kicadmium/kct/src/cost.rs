//! Offline manufacturing cost model, ported from `cost.estimator`.
use crate::schema::pcb::Pcb;
use serde_json::{json, Value};
use std::collections::BTreeMap;
fn r2(v: f64) -> f64 {
    (v * 100.).round() / 100.
}
fn r4(v: f64) -> f64 {
    (v * 10000.).round() / 10000.
}
pub fn estimate(
    p: &Pcb,
    mfr: &str,
    qty: usize,
    finish: &str,
    color: &str,
    thickness: f64,
) -> Value {
    let s = p.summary().ok();
    let (w, h, layers) = s
        .map(|s| (s.width_mm, s.height_mm, s.copper_layers))
        .unwrap_or((50., 50., 2));
    let area = w * h / 100.;
    let layer_mult = match layers {
        4 => 1.8,
        6 => 2.5,
        8 => 3.5,
        10 => 5.,
        12 => 6.5,
        14 => 8.,
        16 => 10.,
        _ => 1.,
    };
    let area_cost = area * 0.02 * qty as f64;
    let layer_cost = 2. * (layer_mult - 1.) * qty as f64;
    let finish_cost = match finish {
        "enig" => 1.,
        "hard_gold" => 30.,
        "immersion_silver" => 5.,
        _ => 0.,
    } * qty as f64;
    let color_cost = match color {
        "white" | "yellow" | "purple" => 2.,
        "matte_black" | "matte_green" => 5.,
        _ => 0.,
    };
    let thick_cost = match thickness {
        v if (v - 0.4).abs() < 1e-6 => 3.,
        v if (v - 0.6).abs() < 1e-6 => 2.,
        v if (v - 2.).abs() < 1e-6 => 2.,
        v if (v - 2.4).abs() < 1e-6 => 5.,
        _ => 0.,
    };
    let disc = if qty >= 500 {
        0.35
    } else if qty >= 200 {
        0.45
    } else if qty >= 100 {
        0.55
    } else if qty >= 50 {
        0.65
    } else if qty >= 30 {
        0.70
    } else if qty >= 20 {
        0.75
    } else if qty >= 10 {
        0.85
    } else {
        1.
    };
    let pcb_total = (2. + area_cost + layer_cost + finish_cost + color_cost + thick_cost) * disc;
    let mut groups: BTreeMap<(String, String), (String, usize)> = BTreeMap::new();
    for f in p
        .footprints()
        .iter()
        .filter(|f| !f.reference.is_empty() && !f.reference.starts_with('#') && !f.dnp)
    {
        let e = groups
            .entry((f.value.clone(), f.name.clone()))
            .or_insert((f.reference.clone(), 0));
        e.1 += 1;
    }
    let mut items = Vec::new();
    let mut categories: BTreeMap<&str, f64> = BTreeMap::new();
    let mut component_total = 0.;
    let mut smt = 0;
    let mut tht = 0;
    let mut bga = 0;
    for ((value, fp), (reference, n)) in &groups {
        let fl = fp.to_lowercase();
        let unit = if ["0201", "0402", "0603"].iter().any(|x| fl.contains(x)) {
            if fl.contains("0201") {
                0.002
            } else if fl.contains("0402") {
                0.003
            } else {
                0.004
            }
        } else if fl.contains("0805") {
            0.005
        } else if fl.contains("1206") {
            0.008
        } else {
            match prefix(reference) {
                "D" | "LED" => 0.02,
                "J" | "P" | "SW" => 0.1,
                "R" => 0.005,
                "C" => 0.008,
                "L" => 0.02,
                "Q" => 0.05,
                "U" => 0.5,
                "Y" => 0.2,
                "F" => 0.05,
                "FB" => 0.01,
                _ => 0.01,
            }
        };
        let ext = unit * *n as f64;
        component_total += ext;
        *categories.entry(category(prefix(reference))).or_default() += ext;
        let is_tht = ["dip", "to-220", "to-92", "sip", "through"]
            .iter()
            .any(|x| fl.contains(x));
        if is_tht {
            tht += n
        } else {
            smt += n
        }
        if fl.contains("bga") {
            bga += n
        }
        items.push(json!({"reference":reference,"value":value,"mpn":null,"lcsc":null,"quantity":n,"unit_cost":r4(unit),"extended_cost":r2(ext),"in_stock":true,"is_basic":false,"pricing_source":"estimated","lookup_unavailable":false}));
    }
    let double = p.footprints().iter().any(|f| f.layer == "B.Cu");
    let smt_cost = smt as f64 * 4. * 0.0017 * qty as f64;
    let tht_cost = tht as f64 * 0.02 * qty as f64;
    let bga_cost = bga as f64 * 0.1 * qty as f64;
    let mult = if double { 1.8 } else { 1. };
    let asm_total = (9.5 + (smt_cost + tht_cost + bga_cost) * mult).max(qty as f64);
    let pcb_unit = pcb_total / qty as f64;
    let asm_unit = asm_total / qty as f64;
    let total = pcb_unit + component_total + asm_unit;
    json!({"manufacturer":mfr,"quantity":qty,"currency":"USD","summary":{"pcb_cost_per_unit":r2(pcb_unit),"component_cost_per_unit":r2(component_total),"assembly_cost_per_unit":r2(asm_unit),"total_per_unit":r2(total),"total_for_quantity":r2(total*qty as f64)},"pcb":{"cost_per_unit":r2(pcb_unit),"total_cost":r2(pcb_total),"breakdown":{"base":2.0,"area":r2(area_cost),"layers":r2(layer_cost),"finish":r2(finish_cost),"color":r2(color_cost),"vias":0.0,"thickness":r2(thick_cost)},"specs":{"width_mm":w,"height_mm":h,"area_cm2":r2(area),"layer_count":layers,"surface_finish":finish,"solder_mask_color":color,"board_thickness_mm":thickness}},"components":{"cost_per_unit":r2(component_total),"total_parts":groups.values().map(|x|x.1).sum::<usize>(),"unique_parts":groups.len(),"breakdown":categories,"items":items},"assembly":{"cost_per_unit":r2(asm_unit),"total_cost":r2(asm_total),"breakdown":{"smt":r2(smt_cost),"through_hole":r2(tht_cost),"setup":9.5,"bga":r2(bga_cost),"fine_pitch":0.0},"specs":{"smt_parts":smt,"through_hole_parts":tht,"unique_parts":groups.len(),"bga_parts":bga,"double_sided":double}},"cost_drivers":[],"optimization_suggestions":[format!("Use basic parts to avoid extended part fees ({} extended parts)",groups.len())]})
}
fn prefix(r: &str) -> &str {
    let n = r.find(|c: char| c.is_ascii_digit()).unwrap_or(r.len());
    &r[..n]
}
fn category(p: &str) -> &'static str {
    match p {
        "R" => "Resistors",
        "C" => "Capacitors",
        "L" => "Inductors",
        "D" => "Diodes",
        "Q" => "Transistors",
        "U" => "ICs",
        "J" | "P" => "Connectors",
        "SW" => "Switches",
        "Y" => "Crystals",
        "F" => "Fuses",
        "LED" => "LEDs",
        "FB" => "Ferrite Beads",
        _ => "Other",
    }
}
