//! Copper-layer stackup helpers (port of `kicad_tools.core.layers`).
//!
//! KiCad's layer *numbering* is not the physical stackup order, so "which
//! layers does this via barrel pass through?" consults the canonical
//! front-to-back name ordering instead of layer indices.

use std::collections::BTreeSet;

use crate::exceptions::ValueError;
use crate::utils::pyrepr::py_str_repr;

/// Canonical copper layer ordering from front to back (`F.Cu`,
/// `In1.Cu`..`In30.Cu`, `B.Cu`).
pub const COPPER_LAYER_ORDER: [&str; 32] = [
    "F.Cu", "In1.Cu", "In2.Cu", "In3.Cu", "In4.Cu", "In5.Cu", "In6.Cu", "In7.Cu", "In8.Cu",
    "In9.Cu", "In10.Cu", "In11.Cu", "In12.Cu", "In13.Cu", "In14.Cu", "In15.Cu", "In16.Cu",
    "In17.Cu", "In18.Cu", "In19.Cu", "In20.Cu", "In21.Cu", "In22.Cu", "In23.Cu", "In24.Cu",
    "In25.Cu", "In26.Cu", "In27.Cu", "In28.Cu", "In29.Cu", "In30.Cu", "B.Cu",
];

/// Stack index of a canonical copper layer name.
pub fn copper_layer_index(name: &str) -> Option<usize> {
    COPPER_LAYER_ORDER.iter().position(|l| *l == name)
}

/// Canonical copper declarations, including `mixed`/`jumper` layer types.
pub fn is_declared_copper_layer(name: &str, layer_type: &str) -> bool {
    copper_layer_index(name).is_some()
        && matches!(layer_type, "signal" | "power" | "mixed" | "jumper")
}

/// Reject a layer that is not a canonical copper name, or not among the
/// board's declared/enabled copper layers.
pub fn validate_copper_layer<I, S>(layer: &str, available_layers: I) -> Result<(), ValueError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if copper_layer_index(layer).is_none() {
        return Err(ValueError::new(format!(
            "{} is not a valid copper layer name. Copper layers are \
             'F.Cu', 'B.Cu', or an inner layer like 'In1.Cu'..'In30.Cu'.",
            py_str_repr(layer)
        )));
    }
    let available: BTreeSet<String> = available_layers
        .into_iter()
        .map(|s| s.as_ref().to_string())
        .collect();
    if !available.contains(layer) {
        let mut names: Vec<&String> = available.iter().collect();
        names.sort_by_key(|n| (copper_layer_index(n).unwrap_or(usize::MAX), (*n).clone()));
        let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        return Err(ValueError::new(format!(
            "Layer {} is not available on this board. Declared copper layers: {}.",
            py_str_repr(layer),
            names.join(", ")
        )));
    }
    Ok(())
}

/// Whether a via barrel passes through `target_layer`: a via spans every
/// copper layer between (and including) its declared endpoints.
pub fn via_spans_layer<S: AsRef<str>>(via_layers: &[S], target_layer: &str) -> bool {
    if via_layers.iter().any(|l| l.as_ref() == target_layer) {
        return true;
    }
    let indices: Vec<usize> = via_layers
        .iter()
        .filter_map(|l| copper_layer_index(l.as_ref()))
        .collect();
    if indices.len() < 2 {
        return false;
    }
    let Some(target) = copper_layer_index(target_layer) else {
        return false;
    };
    let lo = *indices.iter().min().unwrap();
    let hi = *indices.iter().max().unwrap();
    lo <= target && target <= hi
}
