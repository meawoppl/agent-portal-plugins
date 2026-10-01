//! DRC rule implementations for the pure-Rust checker (port of
//! `kicad_tools.validate.rules`).

pub mod ampacity;
pub mod base;
pub mod clearance;
pub mod connectivity;
pub mod connector_access;
pub mod copper_sliver;
pub mod courtyard;
pub mod courtyard_waivers;
pub mod dangling_copper;
pub mod diffpair_clearance_intra;
pub mod diffpair_length_skew;
pub mod diffpair_routing_continuity;
pub mod dimensions;
pub mod edge;
pub mod impedance;
pub mod match_group_length_skew;
pub mod netlist;
pub mod path_ampacity;
pub mod physical_gap;
pub mod pin1_marker;
pub mod placement;
pub mod silkscreen;
pub mod single_pad_net;
pub mod solder_mask;
pub mod via_in_pad;
pub mod via_under_body;
pub mod width_consistency;
pub mod zero_length_segment;
pub mod zone_fill;

pub use base::DRC_TOLERANCE;
