//! Port of `kicad_tools/placement/__init__.py`.

pub mod analyzer;
pub mod bo_strategy;
pub mod cmaes_strategy;
pub mod collision;
pub mod conflict;
pub mod cost;
pub mod cpp;
pub mod cpp_backend;
pub mod drc;
pub mod fixer;
pub mod geometry;
pub mod hv_domains;
pub mod multi_fidelity;
pub mod place_unplaced;
pub mod priors;
pub mod routing;
pub mod seed;
pub mod slide_off;
pub mod strategy;
pub mod vector;
pub mod visualization;
pub mod wirelength;
