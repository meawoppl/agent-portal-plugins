//! Internal footprint keys independent of ambiguous displayed references
//! (port of `kicad_tools.schema.physical_identity`).
//!
//! Upstream takes `schema.pcb.Footprint` objects; this port takes the plain
//! fields it reads (reference, UUID, pad numbers/positions) so it does not
//! depend on the PCB model.

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};

/// Stable per-footprint keys: a unique reference is its own key; repeated
/// references (including repeated empty ones) become
/// `@kct-footprint:uuid:<uuid>` (unique UUID) or `@kct-footprint:index:<i>`,
/// prefixed with extra `@` until they collide with no reference.
pub fn footprint_keys<'a>(footprints: &[(&'a str, &'a str)]) -> Vec<String> {
    let mut refs: HashMap<&str, usize> = HashMap::new();
    let mut uuids: HashMap<&str, usize> = HashMap::new();
    for &(reference, uuid) in footprints {
        *refs.entry(reference).or_default() += 1;
        *uuids.entry(uuid).or_default() += 1;
    }
    let mut reserved: HashSet<String> = refs.keys().map(|r| r.to_string()).collect();
    let mut result = Vec::with_capacity(footprints.len());
    for (index, &(reference, uuid)) in footprints.iter().enumerate() {
        let key = if refs[reference] == 1 {
            reference.to_string()
        } else {
            let identity = if !uuid.is_empty() && uuids[uuid] == 1 {
                format!("uuid:{uuid}")
            } else {
                format!("index:{index}")
            };
            let mut key = format!("@kct-footprint:{identity}");
            while reserved.contains(&key) {
                key.insert(0, '@');
            }
            key
        };
        reserved.insert(key.clone());
        result.push(key);
    }
    result
}

/// A routing component (upstream dict with `ref` and optional `component_id`).
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentRef {
    pub reference: String,
    pub component_id: Option<String>,
}

/// Keys for routing components: carried `component_id`s win, unique
/// references are kept, duplicates get `@kct-footprint:index:<i>`.
pub fn component_keys(components: &[ComponentRef]) -> Result<Vec<String>> {
    let mut refs: HashMap<&str, usize> = HashMap::new();
    for c in components {
        *refs.entry(c.reference.as_str()).or_default() += 1;
    }
    let mut reserved: HashSet<String> = components.iter().map(|c| c.reference.clone()).collect();
    reserved.extend(components.iter().filter_map(|c| c.component_id.clone()));
    let mut result: Vec<String> = Vec::with_capacity(components.len());
    for (index, c) in components.iter().enumerate() {
        let key = if let Some(id) = &c.component_id {
            id.clone()
        } else if refs[c.reference.as_str()] == 1 {
            c.reference.clone()
        } else {
            let mut key = format!("@kct-footprint:index:{index}");
            while reserved.contains(&key) {
                key.insert(0, '@');
            }
            key
        };
        if result.contains(&key) {
            bail!("Duplicate physical component identity {key:?}");
        }
        reserved.insert(key.clone());
        result.push(key);
    }
    Ok(result)
}

/// Reject `REF.PAD` override selectors naming pads on several footprints.
/// `footprints` holds each footprint's reference and pad numbers.
pub fn validate_netlist_selectors<S: AsRef<str>>(
    footprints: &[(&str, Vec<&str>)],
    overrides: impl IntoIterator<Item = S>,
) -> Result<()> {
    let mut owners: HashMap<String, HashSet<usize>> = HashMap::new();
    for (index, (reference, pads)) in footprints.iter().enumerate() {
        for pad in pads {
            owners
                .entry(format!("{reference}.{pad}"))
                .or_default()
                .insert(index);
        }
    }
    let mut ambiguous: Vec<String> = overrides
        .into_iter()
        .map(|k| k.as_ref().to_string())
        .filter(|k| owners.get(k).is_some_and(|o| o.len() > 1))
        .collect();
    ambiguous.sort();
    ambiguous.dedup();
    if !ambiguous.is_empty() {
        bail!(
            "Ambiguous netlist terminal selectors: {}",
            ambiguous.join(", ")
        );
    }
    Ok(())
}

/// KiCad's footprint-local to board offset rotation
/// (`kicad_tools.core.geometry.rotate_pad_offset`: CCW matrix at `-rotation`).
pub fn rotate_pad_offset(local_x: f64, local_y: f64, rotation_deg: f64) -> (f64, f64) {
    let a = (-rotation_deg).to_radians();
    let (sin_a, cos_a) = (a.sin(), a.cos());
    (
        local_x * cos_a - local_y * sin_a,
        local_x * sin_a + local_y * cos_a,
    )
}

/// Board position of a pad occurrence given its footprint's position/rotation.
pub fn physical_pad_position(
    fp_position: (f64, f64),
    fp_rotation: f64,
    pad_position: (f64, f64),
) -> (f64, f64) {
    let (dx, dy) = rotate_pad_offset(pad_position.0, pad_position.1, fp_rotation);
    (fp_position.0 + dx, fp_position.1 + dy)
}
