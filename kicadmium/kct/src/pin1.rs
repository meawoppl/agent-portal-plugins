//! Attach existing board pin-1 circles to their owning footprint.
use crate::{schema::pcb::Pcb, sexp::SExp};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub reference: String,
    pub uuid: String,
}
#[derive(Debug, Serialize)]
pub struct Attached {
    pub reference: String,
    pub uuid: String,
    pub distance_mm: f64,
    pub already_attached: bool,
}
fn string<'a>(s: &'a SExp, field: &str) -> Option<&'a str> {
    s.child_str(field)
}
fn point(s: &SExp, field: &str) -> Result<(f64, f64)> {
    let n = s.get(field).with_context(|| format!("missing {field}"))?;
    let x = n.float_at(0).context("missing x")?;
    let y = n.float_at(1).context("missing y")?;
    if !x.is_finite() || !y.is_finite() {
        bail!("non-finite marker coordinate")
    };
    Ok((x, y))
}
/// All validation occurs on a clone: an invalid entry leaves the entire board intact.
pub fn attach(pcb: &mut Pcb, markers: &[Marker], max_distance: f64) -> Result<Vec<Attached>> {
    if !max_distance.is_finite() || max_distance <= 0. {
        bail!("maximum distance must be finite and positive")
    }
    let mut tree = pcb.sexp().clone();
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for marker in markers {
        if !seen.insert(&marker.uuid) {
            bail!("duplicate marker {}", marker.uuid)
        }
        let fp = pcb
            .footprints()
            .iter()
            .find(|fp| fp.reference == marker.reference)
            .with_context(|| format!("unknown footprint {}", marker.reference))?;
        let pad = fp
            .pads
            .iter()
            .find(|p| p.number == "1")
            .or_else(|| fp.pads.iter().find(|p| p.number == "A1"))
            .context("footprint has no pad 1/A1")?;
        let fp_index = tree
            .children
            .iter()
            .position(|n| n.has_tag("footprint") && string(n, "uuid") == Some(fp.uuid.as_str()))
            .context("footprint UUID not found")?;
        let existing = tree.children[fp_index]
            .children
            .iter()
            .find(|n| n.has_tag("fp_circle") && string(n, "uuid") == Some(marker.uuid.as_str()));
        let already = existing.is_some();
        let mut circle = if let Some(n) = existing {
            n.clone()
        } else {
            tree.children
                .iter()
                .find(|n| n.has_tag("gr_circle") && string(n, "uuid") == Some(marker.uuid.as_str()))
                .cloned()
                .context("marker UUID is not a board circle")?
        };
        let silk = if fp.layer.starts_with('B') {
            "B.SilkS"
        } else {
            "F.SilkS"
        };
        if string(&circle, "layer") != Some(silk) {
            bail!("marker {} is not on {}", marker.uuid, silk)
        }
        if !already {
            let angle = fp.rotation.to_radians();
            let (sin, cos) = angle.sin_cos();
            for field in ["center", "end"] {
                let (x, y) = point(&circle, field)?;
                let x = x - fp.position.0 - pcb.board_origin().0;
                let y = y - fp.position.1 - pcb.board_origin().1;
                *circle.get_mut(field).unwrap() = SExp::list(
                    field,
                    [SExp::atom(x * cos - y * sin), SExp::atom(x * sin + y * cos)],
                );
            }
        }
        let center = point(&circle, "center")?;
        let distance = (center.0 - pad.position.0).hypot(center.1 - pad.position.1);
        if distance > max_distance {
            bail!(
                "marker {} is {:.3} mm from {} pad 1 (maximum {:.3})",
                marker.uuid,
                distance,
                marker.reference,
                max_distance
            )
        }
        if !already {
            circle.name = Some("fp_circle".into());
            tree.children[fp_index].children.push(circle);
            tree.children.retain(|n| {
                !(n.has_tag("gr_circle") && string(n, "uuid") == Some(marker.uuid.as_str()))
            });
        }
        results.push(Attached {
            reference: marker.reference.clone(),
            uuid: marker.uuid.clone(),
            distance_mm: distance,
            already_attached: already,
        });
    }
    *pcb.sexp_mut() = tree;
    pcb.reload_from_tree()?;
    Ok(results)
}
