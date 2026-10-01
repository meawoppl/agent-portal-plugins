//! Autorouter (port of `kicad_tools.router`).
//!
//! Module layout mirrors upstream `kicad_tools/router/*.py`. The upstream
//! C++ backend (`router/cpp/`, `cpp_backend.py`, `evaluators/cpp_astar.py`)
//! is replaced by the native Rust pathfinder; there is no FFI.

pub mod geometry;
pub mod layers;
pub mod primitives;
pub mod pyrandom;
pub mod quantize;
