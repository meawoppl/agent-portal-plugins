//! ERC report parsing and cross-sheet analysis (port of `kicad_tools.erc`).

pub mod cross_sheet;
pub mod report;
pub mod violation;

pub use cross_sheet::{
    check_cross_sheet_duplicates, filter_cross_sheet_global_labels,
    filter_cross_sheet_global_labels_objs, filter_cross_sheet_power_violations,
    filter_phantom_wire_violations, reattribute_symbol_violations,
    reattribute_wire_dangling_violations,
};
pub use report::{parse_json_report, parse_text_report, ERCReport};
pub use violation::{
    erc_type_description, ERCViolation, ERCViolationType, Severity, ERC_BLOCKING_TYPES,
    ERC_CATEGORIES, ERC_NON_BLOCKING_TYPES, ERC_TYPE_DESCRIPTIONS,
};
