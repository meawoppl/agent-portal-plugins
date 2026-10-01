//! Autorouter (port of `kicad_tools.router`).
//!
//! Module layout mirrors upstream `kicad_tools/router/*.py`. The upstream
//! C++ backend (`router/cpp/`, `cpp_backend.py`, `evaluators/cpp_astar.py`)
//! is replaced by the native Rust pathfinder; there is no FFI. `optimizer`
//! is the post-routing trace optimizer (wave D).

pub mod current_paths;
pub mod geometry;
pub mod layers;
pub mod net_class;
pub mod net_names;
pub mod optimizer;
pub mod primitives;
pub mod pyrandom;
pub mod quantize;
pub mod rules;
