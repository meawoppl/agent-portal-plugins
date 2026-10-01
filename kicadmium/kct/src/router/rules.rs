//! Port of `kicad_tools.router.rules` -- STUB (checker-facing types).

/// Upstream `NetClassRouting` (stub; only the checker-facing fields).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetClassRouting {
    pub name: String,
    pub target_ampacity: Option<f64>,
}

/// `{net_name: NetClassRouting}` in insertion order (upstream dict).
pub type NetClassMap = Vec<(String, NetClassRouting)>;
