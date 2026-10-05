//! Port of `kicad_tools.silkscreen`: reference-designator visibility and
//! board markings (`generator`), the reference-designator placement solver
//! behind `place-silk-refs` (`place_refs`), and shared defaults
//! (`silk_defaults`).

pub mod generator;
pub mod place_refs;
pub mod silk_defaults;

pub use generator::{SilkscreenGenerator, SilkscreenResult, SILKSCREEN_LAYERS};
