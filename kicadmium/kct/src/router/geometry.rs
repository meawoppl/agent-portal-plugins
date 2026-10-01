//! Vector geometry primitives for the router (port of
//! `kicad_tools.router.geometry`): re-exports of the canonical
//! implementations in [`crate::core::geometry`].

pub use crate::core::geometry::{
    point_to_segment_distance, segment_clearance, segment_to_segment_distance, segments_intersect,
};
