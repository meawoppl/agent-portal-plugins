//! Object-specific factory spacing (port of
//! `kicad_tools.validate.rules.factory_clearance`): PTH hole to copper.
//! (`check_silk_pad_clearance` lives with the silkscreen port.)

use super::clearance::{collect_zone_fills, pad_on_layer, transform_pad_position, ClearanceRule};
use crate::geometry::shapely::{self as sh, Geom};
use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::pyjson::py_float_repr;
use crate::schema::pcb::{Footprint, Pad, Pcb};
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

/// `_hole_geometry`: the round/slotted drill with offset and pad angle.
pub fn hole_geometry(pad: &Pad, fp: &Footprint) -> Option<Geom> {
    let (mut w, mut h) = (pad.drill, pad.drill);
    if let Some((dw, dh)) = pad.drill_size {
        w = dw;
        h = dh;
    }
    let (ox, oy) = pad.drill_offset;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = (if h < w { h } else { w }) / 2.0;
    let half = (w - h).abs() / 2.0;
    let geom = if half != 0.0 {
        let (a, b) = if w >= h {
            ((-half, 0.0), (half, 0.0))
        } else {
            ((0.0, -half), (0.0, half))
        };
        sh::segment_buffer_q(a, b, r, 64)
    } else {
        sh::point_buffer_q((0.0, 0.0), r, 64)
    };
    let geom = sh::rotate(&sh::translate(&geom, ox, oy), -pad.rotation);
    let (px, py) = transform_pad_position(pad, fp);
    Some(sh::translate(&geom, px, py))
}

/// `check_pth_hole_clearance`.
pub fn check_pth_hole_clearance(pcb: &Pcb, rules: &DesignRules) -> DRCResults {
    let mut results = DRCResults::new();
    if rules.min_pth_hole_to_track_mm.is_none() && rules.min_inner_pth_hole_to_copper_mm.is_none() {
        return results;
    }
    let holes: Vec<(&Footprint, &Pad, Option<Geom>)> = pcb
        .footprints()
        .iter()
        .flat_map(|fp| {
            fp.pads
                .iter()
                .filter(|p| p.pad_type == "thru_hole")
                .map(move |p| (fp, p, hole_geometry(p, fp)))
        })
        .collect();
    let fills = if rules.min_inner_pth_hole_to_copper_mm.is_some() {
        collect_zone_fills(pcb, true)
    } else {
        Vec::new()
    };
    for layer in pcb.copper_layers() {
        let inner = layer.name != "F.Cu" && layer.name != "B.Cu";
        let inner_min = if inner {
            rules.min_inner_pth_hole_to_copper_mm
        } else {
            None
        };
        let minimum = rules
            .min_pth_hole_to_track_mm
            .unwrap_or(0.0)
            .max(inner_min.unwrap_or(0.0));
        if minimum == 0.0 {
            continue;
        }
        let rule = ClearanceRule;
        let elems = rule.collect_elements(pcb, &layer.name);
        let mut copper: Vec<(i64, Geom, String, Option<&Pad>)> = Vec::new();
        for e in &elems {
            let geom = if e.element_type == "segment" {
                let g = &e.geometry;
                Some(sh::segment_buffer_q(
                    (g[0], g[1]),
                    (g[2], g[3]),
                    g[4] / 2.0,
                    64,
                ))
            } else if inner_min.is_some() {
                e.copper_geom().cloned()
            } else {
                continue;
            };
            if let Some(g) = geom {
                copper.push((e.net_number, g, e.reference.clone(), e.source_pad));
            }
        }
        if inner_min.is_some() {
            if let Some((_, fs)) = fills.iter().find(|(l, _)| *l == layer.name) {
                for f in fs {
                    copper.push((
                        f.net_number,
                        f.polygon.clone(),
                        format!("Zone [{}]", f.net_name),
                        None,
                    ));
                }
            }
        }
        if copper.is_empty() {
            continue;
        }
        let tree = StrTree::new(&copper.iter().map(|c| c.1.bounds()).collect::<Vec<_>>());
        for (fp, pad, hole) in &holes {
            let Some(hole) = hole else { continue };
            if !pad_on_layer(pad, &layer.name) {
                continue;
            }
            let Some((x0, y0, x1, y1)) = hole.bounds() else {
                continue;
            };
            for idx in tree.query((x0 - minimum, y0 - minimum, x1 + minimum, y1 + minimum)) {
                let (net, geom, reference, src) = &copper[idx];
                if src.is_some_and(|s| std::ptr::eq(s, *pad))
                    || (*net != 0 && *net == pad.net_number)
                {
                    continue;
                }
                let d = sh::distance(hole, geom);
                if d + DRC_TOLERANCE < minimum {
                    let (lx, ly) = transform_pad_position(pad, fp);
                    results.add(
                        DRCViolation::new(
                            "pth_hole_clearance",
                            "error",
                            format!(
                                "PTH hole to copper clearance {d:.4}mm < {}mm",
                                py_float_repr(minimum)
                            ),
                        )
                        .at(lx, ly)
                        .layer(layer.name.clone())
                        .actual(d)
                        .required(minimum)
                        .items([
                            format!("{}-{} hole", fp.reference, pad.number),
                            reference.clone(),
                        ]),
                    );
                }
            }
        }
    }
    results.rules_checked = 1;
    results
}

/// `check_silk_pad_clearance`: modeled silk strokes vs pad copper /
/// mask apertures on the same side.
pub fn check_silk_pad_clearance(pcb: &Pcb, rules: &DesignRules) -> DRCResults {
    use super::silkscreen::{fp_transform, silk_side, stroke_geometry, SilkGraphic};
    let mut results = DRCResults::new();
    let Some(minimum) = rules.min_silk_to_pad_clearance_mm else {
        return results;
    };
    let mut apertures: Vec<(&str, Geom, String)> = Vec::new();
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            if !matches!(pad.pad_type.as_str(), "smd" | "thru_hole" | "connect") {
                continue;
            }
            let margin = pad
                .solder_mask_margin
                .unwrap_or_else(|| pcb.setup().map_or(0.0, |s| s.pad_to_mask_clearance));
            let Some(copper) = super::clearance::pad_polygon(pad, fp) else {
                continue;
            };
            for side in ["F", "B"] {
                let cu = format!("{side}.Cu");
                if !pad.layers.iter().any(|l| l == "*.Cu" || *l == cu) {
                    continue;
                }
                let mask = format!("{side}.Mask");
                let exposed = pad.layers.iter().any(|l| l == "*.Mask" || *l == mask);
                let g = if exposed {
                    sh::buffer_polygon(&copper, margin.max(0.0))
                } else {
                    copper.clone()
                };
                apertures.push((side, g, format!("{}-{}", fp.reference, pad.number)));
            }
        }
    }
    let by_side = |side: &str| -> Vec<(sh::Prepared, String)> {
        apertures
            .iter()
            .filter(|(s, g, _)| *s == side && !g.is_empty())
            .map(|(_, g, l)| (sh::Prepared::new(g.clone()), l.clone()))
            .collect()
    };
    let sides = [("F", by_side("F")), ("B", by_side("B"))];
    let trees: Vec<StrTree> = sides
        .iter()
        .map(|(_, e)| StrTree::new(&e.iter().map(|(g, _)| g.bounds()).collect::<Vec<_>>()))
        .collect();
    type Stroke<'a> = (SilkGraphic<'a>, Option<&'a Footprint>, String, (f64, f64));
    let mut strokes: Vec<Stroke<'_>> = Vec::new();
    for fp in pcb.footprints() {
        for g in &fp.graphics {
            strokes.push((
                SilkGraphic::from(g),
                Some(fp),
                fp.reference.clone(),
                fp.position,
            ));
        }
    }
    for g in pcb.graphics() {
        strokes.push((SilkGraphic::from(g), None, "board".to_string(), g.start));
    }
    for (g, fp, reference, location) in strokes {
        let Some(side) = silk_side(g.layer) else {
            continue;
        };
        let si = if side == "F" { 0 } else { 1 };
        if sides[si].1.is_empty() {
            continue;
        }
        let geom = match fp {
            Some(fp) => {
                let t = fp_transform(fp);
                stroke_geometry(&g, Some(&t))
            }
            None => stroke_geometry(&g, None),
        };
        let Some(geom) = geom else { continue };
        let Some((x0, y0, x1, y1)) = geom.bounds() else {
            continue;
        };
        let mut silk_item = format!("{reference} {}", g.graphic_type);
        if !g.uuid.is_empty() {
            silk_item = format!("{silk_item} {{{}}}", g.uuid);
        }
        for idx in trees[si].query((x0 - minimum, y0 - minimum, x1 + minimum, y1 + minimum)) {
            let (ap, pad_ref) = &sides[si].1[idx];
            let d = sh::distance_prep(&geom, ap);
            if d + DRC_TOLERANCE < minimum {
                results.add(
                    DRCViolation::new(
                        "silk_pad_clearance",
                        "error",
                        format!(
                            "Silk to pad clearance {d:.4}mm < {}mm",
                            py_float_repr(minimum)
                        ),
                    )
                    .at(location.0, location.1)
                    .layer(g.layer.to_string())
                    .actual(d)
                    .required(minimum)
                    .items([silk_item.clone(), pad_ref.clone()]),
                );
            }
        }
    }
    results.rules_checked = 1;
    results
}
