//! Port of `kicad_tools.router` (in progress): support modules for the
//! pure-Rust DRC checker plus wave D's trace optimizer and the primitives
//! it needs (`layers`, `primitives`, `quantize`). The autorouter is Wave F.

pub mod current_paths;
pub mod layers;
pub mod net_class;
pub mod net_names;
pub mod optimizer;
pub mod primitives;
pub mod quantize;
pub mod rules;
