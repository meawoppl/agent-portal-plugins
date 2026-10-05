//! Port of `kicad_tools.recovery`: intelligent failure recovery for routing
//! and placement. Failure analyses (`types`) feed a [`StrategyGenerator`]
//! that ranks concrete resolution strategies, a [`StrategyApplicator`] that
//! applies placement strategies to a [`crate::schema::pcb::Pcb`], and a
//! [`PatternMatcher`] for best-practice suggestions.

pub mod applicator;
pub mod patterns;
pub mod strategy;
pub mod types;

pub use applicator::{ApplicationResult, StrategyApplicator};
pub use patterns::PatternMatcher;
pub use strategy::StrategyGenerator;
pub use types::{
    Action, BlockingElement, Difficulty, FailureAnalysis, FailureCause, PathAttempt, Rectangle,
    ResolutionStrategy, SideEffect, StrategyType,
};
