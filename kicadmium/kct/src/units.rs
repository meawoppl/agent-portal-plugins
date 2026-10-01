//! mm/mils display formatting (port of `kicad_tools.units`). All internal
//! lengths are millimetres.

use std::sync::RwLock;

pub const MM_PER_MIL: f64 = 0.0254;
pub const UNITS_ENV_VAR: &str = "KICAD_TOOLS_UNITS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitSystem {
    #[default]
    Mm,
    Mils,
}

impl UnitSystem {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mm" | "millimeters" | "millimeter" => Some(Self::Mm),
            "mils" | "mil" | "thou" | "thousandths" => Some(Self::Mils),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Mils => "mils",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitFormatter {
    pub system: UnitSystem,
    pub precision_mm: usize,
    pub precision_mils: usize,
}

impl Default for UnitFormatter {
    fn default() -> Self {
        Self::new(UnitSystem::Mm)
    }
}

impl UnitFormatter {
    pub fn new(system: UnitSystem) -> Self {
        Self {
            system,
            precision_mm: 3,
            precision_mils: 1,
        }
    }

    /// CLI flag > `KICAD_TOOLS_UNITS` > mm.
    pub fn resolve(cli: Option<&str>) -> Self {
        let system = cli
            .and_then(UnitSystem::parse)
            .or_else(|| {
                std::env::var(UNITS_ENV_VAR)
                    .ok()
                    .as_deref()
                    .and_then(UnitSystem::parse)
            })
            .unwrap_or_default();
        Self::new(system)
    }

    fn display(&self, mm: f64) -> (f64, usize) {
        match self.system {
            UnitSystem::Mils => (mm / MM_PER_MIL, self.precision_mils),
            UnitSystem::Mm => (mm, self.precision_mm),
        }
    }

    pub fn format(&self, mm: f64) -> String {
        let (v, p) = self.display(mm);
        format!("{v:.p$} {}", self.system.name())
    }

    pub fn format_bare(&self, mm: f64) -> String {
        let (v, p) = self.display(mm);
        format!("{v:.p$}")
    }

    pub fn format_compact(&self, mm: f64) -> String {
        let (v, p) = self.display(mm);
        format!("{v:.p$}{}", self.system.name())
    }

    pub fn format_range(&self, min_mm: f64, max_mm: f64) -> String {
        let (a, p) = self.display(min_mm);
        let (b, _) = self.display(max_mm);
        format!("{a:.p$}-{b:.p$} {}", self.system.name())
    }

    pub fn format_coordinate(&self, x_mm: f64, y_mm: f64) -> String {
        let (x, p) = self.display(x_mm);
        let (y, _) = self.display(y_mm);
        format!("({x:.p$}, {y:.p$}) {}", self.system.name())
    }

    pub fn format_delta(&self, mm: f64) -> String {
        let (v, p) = self.display(mm);
        format!("{v:+.p$} {}", self.system.name())
    }

    pub fn format_comparison(&self, actual_mm: f64, required_mm: f64, show_delta: bool) -> String {
        let (a, r) = (self.format(actual_mm), self.format(required_mm));
        if show_delta {
            format!(
                "{a} (required: {r}, need {})",
                self.format_delta(required_mm - actual_mm)
            )
        } else {
            format!("{a} (required: {r})")
        }
    }

    pub fn to_display(&self, mm: f64) -> f64 {
        self.display(mm).0
    }

    pub fn from_display(&self, v: f64) -> f64 {
        match self.system {
            UnitSystem::Mils => v * MM_PER_MIL,
            UnitSystem::Mm => v,
        }
    }
}

static CURRENT: RwLock<Option<UnitFormatter>> = RwLock::new(None);

/// Session formatter set by the CLI `--units` flag.
pub fn set_current(formatter: UnitFormatter) {
    *CURRENT.write().unwrap() = Some(formatter);
}

pub fn current() -> UnitFormatter {
    CURRENT.read().unwrap().unwrap_or_default()
}

pub fn format_length(mm: f64) -> String {
    current().format(mm)
}

pub fn format_coordinate(x_mm: f64, y_mm: f64) -> String {
    current().format_coordinate(x_mm, y_mm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let mm = UnitFormatter::new(UnitSystem::Mm);
        let mils = UnitFormatter::new(UnitSystem::Mils);
        assert_eq!(mm.format(0.254), "0.254 mm");
        assert_eq!(mils.format(0.254), "10.0 mils");
        assert_eq!(mm.format_delta(0.05), "+0.050 mm");
        assert_eq!(
            mm.format_comparison(0.15, 0.2, true),
            "0.150 mm (required: 0.200 mm, need +0.050 mm)"
        );
    }
}
