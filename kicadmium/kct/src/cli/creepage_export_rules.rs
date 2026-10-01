//! `kct creepage-export-rules` (Wave E; port in progress).

use std::ffi::OsString;

use anyhow::Result;

use super::Globals;

pub fn run(_args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    eprintln!("kct creepage-export-rules: not yet ported to Rust (wave E; see kct/PORTING.md)");
    Ok(super::EXIT_NOT_PORTED)
}
