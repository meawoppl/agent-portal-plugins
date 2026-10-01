//! Port of `kicad_tools.router.current_paths` -- STUB (checker-facing types).

/// Upstream `CurrentPathSpec` (stub).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurrentPathSpec {
    pub name: String,
    pub net_name: String,
    pub continuous_a: f64,
}
