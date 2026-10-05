//! Port of `kicad_tools.operations`: schematic net tracing (`net_ops`),
//! netlist parsing/export (`netlist`), pin mapping (`pinmap`) and symbol
//! instance edits (`symbol_ops`).

pub mod net_ops;
pub mod netlist;
pub mod pinmap;
pub mod symbol_ops;

pub use net_ops::{find_net, trace_nets, Net, NetConnection, NetTracer};
pub use netlist::{
    export_netlist, find_kicad_cli, ComponentPin, Netlist, NetlistComponent, NetlistNet,
};
pub use pinmap::{
    compare_schematic_symbols, compare_symbols, extract_pins_from_sexp, load_symbol_from_file,
    load_symbol_from_schematic, match_pins, MappingResult, Pin, PinMapping,
};
pub use symbol_ops::{
    add_symbol_pin, clear_symbol_pins, create_replacement_symbol, find_symbol_by_reference,
    get_symbol_lib_id, get_symbol_pins, replace_symbol_lib_id, update_symbol_pins,
    SymbolReplacement,
};
