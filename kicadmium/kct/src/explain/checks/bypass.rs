//! Nearest-decoupler placement checks. Never compare every cap to every pin.
use super::mistake;
use crate::explain::mistakes::{
    distance, is_bypass_cap, is_ground_net, is_power_net, CheckIncomplete, Mistake,
    MistakeCategory, MistakeCheck,
};
use crate::schema::pcb::Pcb;
use std::collections::BTreeMap;

pub const MAX_BYPASS_DISTANCE_MM: f64 = 3.0;
pub const BYPASS_WARNING_DISTANCE_MM: f64 = 5.0;
#[derive(Debug, Default, Clone, Copy)]
pub struct BypassCapDistanceCheck;
impl MistakeCheck for BypassCapDistanceCheck {
    fn name(&self) -> &'static str {
        "BypassCapDistanceCheck"
    }
    fn category(&self) -> MistakeCategory {
        MistakeCategory::BypassCap
    }
    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        type CapPad<'a> = (&'a str, (f64, f64));
        let mut caps: BTreeMap<&str, Vec<CapPad<'_>>> = BTreeMap::new();
        for cap in pcb.footprints().iter().filter(|fp| {
            is_bypass_cap(&fp.reference, &fp.value)
                && fp.pads.len() == 2
                && fp.pads.iter().any(|p| is_ground_net(&p.net_name))
        }) {
            for pad in cap.pads.iter().filter(|p| is_power_net(&p.net_name)) {
                if let Some(at) = pcb.get_pad_position(&cap.reference, &pad.number) {
                    caps.entry(&pad.net_name)
                        .or_default()
                        .push((&cap.reference, at));
                }
            }
        }
        // One finding per capacitor/net; preserve the worst served IC pin.
        let mut selected = BTreeMap::new();
        for ic in super::decoupling::find_ics(pcb) {
            for pad in ic.pads.iter().filter(|p| is_power_net(&p.net_name)) {
                let Some(at) = pcb.get_pad_position(&ic.reference, &pad.number) else {
                    continue;
                };
                let Some((cap, cap_at, dist)) = caps
                    .get(pad.net_name.as_str())
                    .into_iter()
                    .flatten()
                    .map(|(cap, pos)| (*cap, *pos, distance(at, *pos)))
                    .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(b.0)))
                else {
                    continue;
                };
                if dist <= MAX_BYPASS_DISTANCE_MM {
                    continue;
                }
                let entry = selected.entry((cap, pad.net_name.as_str())).or_insert((
                    dist,
                    ic.reference.as_str(),
                    pad.number.as_str(),
                    cap_at,
                ));
                if dist > entry.0 {
                    *entry = (dist, &ic.reference, &pad.number, cap_at);
                }
            }
        }
        Ok(selected.into_iter().map(|((cap, net), (dist, ic, pin, at))| {
            let warning = dist > BYPASS_WARNING_DISTANCE_MM;
            mistake(MistakeCategory::BypassCap, if warning { "warning" } else { "info" },
                if warning { "Bypass capacitor too far from power pin" } else { "Bypass capacitor placement could be improved" },
                vec![cap.into(), ic.into()], Some(at),
                format!("Nearest decoupler {cap} is {dist:.1}mm from {ic} pin {pin} ({net}); recommended maximum is {MAX_BYPASS_DISTANCE_MM:.1}mm. This is the worst served pin for this capacitor/net."),
                format!("Move {cap} closer or add a local decoupler for {ic} pin {pin}, with short connections to {net} and ground."),
                "docs/mistakes/bypass-cap-placement.md")
        }).collect())
    }
}
