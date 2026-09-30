//! kct: a Rust port of [rjwalters/kicad-tools](https://github.com/rjwalters/kicad-tools)
//! (MIT, Copyright (c) 2024 RJ Walters). See `PORTING.md` for the module map
//! and port status.
//!
//! Layout mirrors the Python package (`kicad_tools/<pkg>` -> `kct::<pkg>`):
//! foundations (`sexp`, `units`, `core`, `schema`, `manufacturers`) plus one
//! module per CLI command family under `cli`.

pub mod cli;
pub mod footprints;
pub mod fsutil;
pub mod manufacturers;
pub mod parts;
pub mod sexp;
pub mod units;

pub use anyhow::{Error, Result};
pub use sexp::{parse, parse_file, Document, SExp, Value};

/// Version of upstream kicad-tools this port tracks.
pub const UPSTREAM_VERSION: &str = "0.22.0";
