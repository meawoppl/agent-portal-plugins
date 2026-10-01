//! Doc-drift lint (port of `kicad_tools.validate.doc_drift`) -- STUB.

use std::path::Path;

use crate::validate::violations::DRCResults;

pub fn check_doc_drift(_pcb_path: &Path) -> DRCResults {
    DRCResults::new()
}
