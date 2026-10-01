//! kct: a Rust port of [rjwalters/kicad-tools](https://github.com/rjwalters/kicad-tools)
//! (MIT, Copyright (c) 2024 RJ Walters). See `PORTING.md` for the module map
//! and port status.
//!
//! Layout mirrors the Python package (`kicad_tools/<pkg>` -> `kct::<pkg>`):
//! foundations (`sexp`, `units`, `core`, `schema`, `manufacturers`) plus one
//! module per CLI command family under `cli`.

pub mod analysis;
pub mod cli;
pub mod config;
pub mod constraints;
pub mod core;
pub mod cost;
pub mod creepage;
pub mod drc;
pub mod erc;
pub mod exceptions;
pub mod explain;
pub mod feedback;
pub mod footprints;
pub mod fsutil;
pub mod geometry;
pub mod intent;
pub mod ipc;
pub mod lvs;
pub mod manufacturers;
pub mod optim;
pub mod parts;
pub mod patterns;
pub mod pcb;
pub mod physics;
pub mod progress;
pub mod project;
pub mod pyjson;
pub mod router;
pub mod schema;
pub mod sexp;
pub mod sidecars;
pub mod stitching;
pub mod sync;
pub mod transaction;
pub mod units;
pub mod utils;
pub mod validate;
pub mod zones;

pub use anyhow::{Error, Result};
pub use sexp::{parse, parse_file, Document, SExp, Value};

/// Version of upstream kicad-tools this port tracks.
pub const UPSTREAM_VERSION: &str = "0.22.0";
