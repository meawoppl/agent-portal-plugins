//! Port of the `Layer` alias from `kicad_tools.router.layers`.
//!
//! Upstream re-exports `kicad_tools.core.types.CopperLayer as Layer`; the
//! stack/via definitions in that module are ported with the router core.

pub use crate::core::types::CopperLayer as Layer;
