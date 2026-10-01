//! Autorouter (port of `kicad_tools.router`).
//!
//! Module layout mirrors upstream `kicad_tools/router/*.py`. The upstream
//! C++ backend (`router/cpp/`, `cpp_backend.py`, `evaluators/cpp_astar.py`)
//! is replaced by the native Rust pathfinder; there is no FFI. `optimizer`
//! is the post-routing trace optimizer (wave D).

pub mod current_paths;
pub mod diffpair;
pub mod diffpair_detection;
pub mod geometry;
pub mod kelvin;
pub mod layers;
pub mod match_group_detection;
pub mod net_class;
pub mod net_names;
pub mod optimizer;
pub mod preflight;
pub mod primitives;
pub mod pyrandom;
pub mod quantize;
pub mod rules;
pub mod via_in_pad_eligibility;
