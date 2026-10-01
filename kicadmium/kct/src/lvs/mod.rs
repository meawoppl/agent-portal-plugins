//! Live LVS for `kct check`'s meta rollup (port in progress of the
//! `kicad_tools.lvs` pieces `check_cmd._lvs_subcheck` uses).

use std::path::Path;

use crate::pyjson::Json;

/// `(status, detail, data)` of the LVS sub-check.
pub type LvsOutcome = (&'static str, String, Option<Json>);

/// Run label + copper LVS. Errors are the sub-check's FAILED detail.
pub fn lvs_subcheck(_sch: &Path, _pcb: &Path) -> Result<LvsOutcome, String> {
    Err("LVS comparator raised NotImplementedError: native LVS not ported yet".into())
}
