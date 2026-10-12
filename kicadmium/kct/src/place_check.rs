//! Placement-only preflight on a spatial subset plus served power pins.
use crate::{
    edit_plan::{Operation, Plan},
    schema::pcb::Pcb,
    validate::{DRCChecker, DRCCheckerOptions},
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
type Box2 = (f64, f64, f64, f64);
fn intersects(a: Box2, b: Box2) -> bool {
    a.0 <= b.2 && a.2 >= b.0 && a.1 <= b.3 && a.3 >= b.1
}
fn footprint_box(p: &Pcb, reference: &str) -> Box2 {
    let fp = p.get_footprint(reference).unwrap();
    let mut b = (fp.position.0, fp.position.1, fp.position.0, fp.position.1);
    if let Some(c) = crate::geometry::pcb_adapters::courtyard_geom(
        fp,
        if fp.layer.starts_with('B') { "B" } else { "F" },
    )
    .and_then(|g| g.bounds())
    {
        b = c;
    }
    for pad in &fp.pads {
        if let Some((x, y)) = p.get_pad_position(reference, &pad.number) {
            let r = pad.size.0.hypot(pad.size.1) / 2.;
            b = (
                b.0.min(x - r),
                b.1.min(y - r),
                b.2.max(x + r),
                b.3.max(y + r),
            );
        }
    }
    // Reference/value labels can sit well outside pads and courtyards. Include
    // their full ink bounds so moving a label over a remote via is checked.
    use crate::validate::rules::silkscreen;
    let transform = silkscreen::fp_transform(fp);
    let mut extend = |bounds: Box2| {
        b = (
            b.0.min(bounds.0),
            b.1.min(bounds.1),
            b.2.max(bounds.2),
            b.3.max(bounds.3),
        );
    };
    for text in fp
        .texts
        .iter()
        .filter(|t| !t.hidden && t.layer.ends_with("SilkS"))
    {
        if let Some(bounds) = silkscreen::text_bbox_geometry(
            &text.text,
            text.font_size,
            text.font_thickness,
            transform(text.position),
        )
        .and_then(|g| g.bounds())
        {
            extend(bounds);
        }
    }
    for graphic in fp.graphics.iter().filter(|g| g.layer.ends_with("SilkS")) {
        if let Some(bounds) =
            silkscreen::stroke_geometry(&silkscreen::SilkGraphic::from(graphic), Some(&transform))
                .and_then(|g| g.bounds())
        {
            extend(bounds);
        }
    }
    b
}
fn local_board(p: &Pcb, regions: &[Box2]) -> Result<Pcb> {
    let touches = |b| regions.iter().any(|r| intersects(*r, b));
    let mut ids = HashSet::new();
    for fp in p.footprints() {
        if touches(footprint_box(p, &fp.reference)) {
            ids.insert(fp.uuid.clone());
        }
    }
    for s in p.segments() {
        let w = s.width / 2.;
        if touches((
            s.start.0.min(s.end.0) - w,
            s.start.1.min(s.end.1) - w,
            s.start.0.max(s.end.0) + w,
            s.start.1.max(s.end.1) + w,
        )) {
            ids.insert(s.uuid.clone());
        }
    }
    for v in p.vias() {
        let r = v.size / 2.;
        if touches((
            v.position.0 - r,
            v.position.1 - r,
            v.position.0 + r,
            v.position.1 + r,
        )) {
            ids.insert(v.uuid.clone());
        }
    }
    let mut tree = p.sexp().clone();
    tree.children.retain(|n| {
        !matches!(n.tag(), Some("footprint" | "segment" | "via"))
            || n.child_str("uuid").is_none_or(|id| ids.contains(id))
    });
    Pcb::new(tree, p.path().map(std::path::Path::to_path_buf))
}
pub fn check(original: &Pcb, plan: &Plan, max_distance: f64) -> Result<Value> {
    check_profile(original, plan, max_distance, false)
}
pub fn check_profile(
    original: &Pcb,
    plan: &Plan,
    max_distance: f64,
    same_side: bool,
) -> Result<Value> {
    if !max_distance.is_finite() || max_distance <= 0. {
        bail!("decoupling.max_pin_distance_mm must be positive")
    }
    if plan
        .operations
        .iter()
        .any(|op| !matches!(op, Operation::Move { .. }))
    {
        bail!("place-check accepts move operations only; use edit --dry-run for copper edits")
    }
    let moved: HashSet<_> = plan
        .operations
        .iter()
        .filter_map(|op| {
            if let Operation::Move { reference, .. } = op {
                Some(reference.clone())
            } else {
                None
            }
        })
        .collect();
    let candidate = crate::edit_plan::apply(original, plan)?;
    let mut regions = Vec::new();
    for reference in &moved {
        for p in [original, &candidate] {
            let b = footprint_box(p, reference);
            regions.push((b.0 - 2., b.1 - 2., b.2 + 2., b.3 + 2.));
        }
    }
    let local = local_board(&candidate, &regions)?;
    let checker = DRCChecker::new(&local, DRCCheckerOptions::default())?;
    let mut native = checker.check_courtyard_overlap().violations;
    native.extend(checker.check_clearances().violations);
    native.extend(checker.check_silkscreen().violations);
    native.extend(checker.check_segment_zone_clearances().violations);
    native.extend(checker.check_via_zone_clearances().violations);
    let origin = candidate.board_origin();
    let mut findings:Vec<_>=native.into_iter().filter(|f|f.items.iter().any(|r|moved.contains(r))||f.location.is_some_and(|(x,y)|regions.iter().any(|r|intersects(*r,(x-origin.0,y-origin.1,x-origin.0,y-origin.1))))).map(|f|json!({"rule":f.rule_id,"severity":f.severity,"message":f.message,"at":f.location,"refs":f.items,"nets":f.nets})).collect();
    use crate::explain::mistakes::{is_bypass_cap, is_ground_net, is_power_net};
    let caps: Vec<_> = candidate
        .footprints()
        .iter()
        .filter(|f| {
            is_bypass_cap(&f.reference, &f.value)
                && f.pads.len() == 2
                && f.pads.iter().any(|p| is_ground_net(&p.net_name))
        })
        .collect();
    for ic in candidate
        .footprints()
        .iter()
        .filter(|f| f.reference.starts_with('U') || f.reference.starts_with("IC"))
    {
        for pad in ic.pads.iter().filter(|p| {
            is_power_net(&p.net_name)
                || caps.iter().any(|c| {
                    c.pads
                        .iter()
                        .any(|cp| cp.net_name == p.net_name && !is_ground_net(&cp.net_name))
                })
        }) {
            let at = candidate
                .get_pad_position(&ic.reference, &pad.number)
                .unwrap();
            let nearest = caps
                .iter()
                .filter(|cap| !same_side || cap.layer == ic.layer)
                .flat_map(|cap| {
                    cap.pads
                        .iter()
                        .filter(|p| p.net_name == pad.net_name)
                        .map(|p| {
                            (
                                *cap,
                                candidate
                                    .get_pad_position(&cap.reference, &p.number)
                                    .unwrap(),
                            )
                        })
                })
                .map(|(cap, pos)| (cap, (at.0 - pos.0).hypot(at.1 - pos.1)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            // Include a moved cap even if another cap became the nearest after moving it away.
            let served_before = caps
                .iter()
                .filter(|c| moved.contains(&c.reference))
                .any(|c| c.pads.iter().any(|p| p.net_name == pad.net_name));
            if !moved.contains(&ic.reference) && !served_before {
                continue;
            }
            if nearest.is_none_or(|(_, d)| d > max_distance) {
                findings.push(json!({"rule":"decoupling.max_pin_distance","severity":"error","refs":[ic.reference],"nets":[pad.net_name],"at":[at.0+origin.0,at.1+origin.1],"pin":pad.number,"nearest_cap":nearest.map(|(c,_)|c.reference.clone()),"distance_mm":nearest.map(|(_,d)|d),"required_mm":max_distance,"message":"No decoupler within the configured power-pin distance"}));
            }
        }
    }
    Ok(
        json!({"schema":"kct.place-check/v1","ok":findings.is_empty(),"moved":moved.into_iter().collect::<std::collections::BTreeSet<_>>(),"checked_footprints":local.footprints().len(),"findings":findings,"written":false,"requires_full_drc":true}),
    )
}
