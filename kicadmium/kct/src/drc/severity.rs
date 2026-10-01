//! Severity enums (port of `kicad_tools.core.types.Severity` / `ERCSeverity`
//! and `core.severity.SeverityMixin.from_string`).
// TODO(foundation): re-export from core::types once merged

use std::fmt;

/// Validation severity levels (DRC, validation, general reporting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Severity {
    #[default]
    Error,
    Warning,
    Info,
}

impl Severity {
    pub const ALL: [Severity; 3] = [Severity::Error, Severity::Warning, Severity::Info];

    pub fn value(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }

    /// `SeverityMixin.from_string`: substring match on error/warning/info,
    /// defaulting to the last member (`INFO`).
    pub fn from_string(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        if s.contains("error") {
            Severity::Error
        } else if s.contains("warning") {
            Severity::Warning
        } else {
            Severity::Info
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.value())
    }
}

/// ERC-specific severity levels with exclusion support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ERCSeverity {
    #[default]
    Error,
    Warning,
    Exclusion,
}

impl ERCSeverity {
    pub fn value(self) -> &'static str {
        match self {
            ERCSeverity::Error => "error",
            ERCSeverity::Warning => "warning",
            ERCSeverity::Exclusion => "exclusion",
        }
    }

    /// `SeverityMixin.from_string`: error/warning/exclu substring match,
    /// defaulting to the last member (`EXCLUSION`).
    pub fn from_string(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        if s.contains("error") {
            ERCSeverity::Error
        } else if s.contains("warning") {
            ERCSeverity::Warning
        } else {
            ERCSeverity::Exclusion
        }
    }
}

impl fmt::Display for ERCSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_string_matches_mixin() {
        assert_eq!(Severity::from_string("ERROR"), Severity::Error);
        assert_eq!(Severity::from_string(" Warning "), Severity::Warning);
        assert_eq!(Severity::from_string("info"), Severity::Info);
        assert_eq!(Severity::from_string("bogus"), Severity::Info);
        assert_eq!(ERCSeverity::from_string("excluded"), ERCSeverity::Exclusion);
        assert_eq!(ERCSeverity::from_string("bogus"), ERCSeverity::Exclusion);
    }
}
