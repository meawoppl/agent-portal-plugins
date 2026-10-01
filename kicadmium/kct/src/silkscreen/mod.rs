//! Port of `kicad_tools.silkscreen`: reference-designator visibility and
//! board markings (`generator`), plus shared defaults (`silk_defaults`).
//!
//! The collision-screened reference placer (`place_refs`) lives with its
//! command in `cli::place_silk_refs`.

pub mod generator;
pub mod silk_defaults;

pub use generator::{SilkscreenGenerator, SilkscreenResult, SILKSCREEN_LAYERS};
