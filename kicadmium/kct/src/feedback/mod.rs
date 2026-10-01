//! Feedback and suggestions for DRC/ERC errors (port of `kicad_tools.feedback`).

pub mod suggestions;

pub use suggestions::{
    generate_drc_suggestions, generate_erc_suggestions, AnyViolation, FixSuggestionGenerator,
};
