//! Shared severity parsing (port of `kicad_tools.core.severity`).

/// Port of `SeverityMixin.from_string`: substring matching on `error`,
/// `warning`, then (if the enum has them) `exclu` and `info`; otherwise
/// `default`, else the enum's last member.
pub trait SeverityMixin: Sized + Copy + 'static {
    const ERROR: Self;
    const WARNING: Self;
    const EXCLUSION: Option<Self> = None;
    const INFO: Option<Self> = None;
    /// Members in declaration order.
    const MEMBERS: &'static [Self];

    fn from_string(s: &str) -> Self {
        Self::from_string_or(s, None)
    }

    fn from_string_or(s: &str, default: Option<Self>) -> Self {
        let lower = s.trim().to_lowercase();
        if lower.contains("error") {
            return Self::ERROR;
        }
        if lower.contains("warning") {
            return Self::WARNING;
        }
        if let Some(ex) = Self::EXCLUSION {
            if lower.contains("exclu") {
                return ex;
            }
        }
        if let Some(info) = Self::INFO {
            if lower.contains("info") {
                return info;
            }
        }
        default.unwrap_or_else(|| *Self::MEMBERS.last().unwrap_or(&Self::WARNING))
    }
}
