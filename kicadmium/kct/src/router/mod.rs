//! Router support modules (port of the pieces of `kicad_tools.router` the
//! pure-Rust DRC checker consumes). The autorouter itself is Wave F.

pub mod current_paths;
pub mod diffpair;
pub mod net_class;
pub mod net_names;
pub mod rules;
