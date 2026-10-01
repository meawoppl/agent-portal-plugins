//! Port of `kicad_tools.router` (in progress).
//!
//! Wave D landed only what the trace optimizer needs: [`layers`] (the
//! `Layer` alias), [`primitives`] (`Segment`/`Via`/`Route`), [`quantize`]
//! (45-degree helpers), and [`optimizer`]. The router core (grid,
//! pathfinder, ...) is ported separately.

pub mod layers;
pub mod optimizer;
pub mod primitives;
pub mod quantize;
