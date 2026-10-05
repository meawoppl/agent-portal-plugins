//! Port of `kicad_tools.router.optimizer`: post-routing trace cleanup
//! (collinear merge, zigzag/staircase removal, 45-degree chamfers,
//! PullTight, via minimization), consolidation, and serpentine tuning.

pub mod algorithms;
pub mod chain;
pub mod collision;
pub mod config;
pub mod consolidate;
pub mod geometry;
pub mod pcb;
pub mod serpentine;
pub mod trace;
pub mod via_optimizer;

pub use collision::{
    make_collision_checker, CollisionChecker, CollisionGrid, GridCollisionChecker,
    VectorCollisionChecker,
};
pub use config::{OptimizationConfig, OptimizationStats};
pub use consolidate::{
    consolidate_net_routes, consolidate_routes_grid_synced, consolidate_segments,
    ConsolidationStats,
};
pub use serpentine::{
    add_serpentine, tune_match_group, SerpentineConfig, SerpentineGenerator, SerpentineResult,
    SerpentineStyle,
};
pub use trace::{
    apply_route_transform_grid_synced, optimize_routes_grid_synced, PairwiseRouteGate,
    TraceOptimizer,
};
pub use via_optimizer::{
    optimize_route_vias, LayerConnectivityError, ViaOptimizationConfig, ViaOptimizationStats,
    ViaOptimizer,
};
