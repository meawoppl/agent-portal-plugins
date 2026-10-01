//! Component value parsing (the `parse_component_value` slice of
//! `kicad_tools.cost.suggest`, used by the schematic/PCB reconciler).

use std::sync::LazyLock;

use regex::Regex;

use crate::utils::pyfmt::format_g;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentType {
    Resistor,
    Capacitor,
    Inductor,
    Led,
    Diode,
    Transistor,
    Ic,
    Connector,
    Crystal,
    Fuse,
    Other,
}

impl ComponentType {
    pub fn value(self) -> &'static str {
        match self {
            ComponentType::Resistor => "resistor",
            ComponentType::Capacitor => "capacitor",
            ComponentType::Inductor => "inductor",
            ComponentType::Led => "led",
            ComponentType::Diode => "diode",
            ComponentType::Transistor => "transistor",
            ComponentType::Ic => "ic",
            ComponentType::Connector => "connector",
            ComponentType::Crystal => "crystal",
            ComponentType::Fuse => "fuse",
            ComponentType::Other => "other",
        }
    }
}

/// Parsed component value.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedValue {
    pub raw_value: String,
    pub component_type: ComponentType,
    pub numeric_value: Option<f64>,
    pub unit: String,
    pub tolerance: String,
    pub voltage_rating: String,
    pub search_terms: Vec<String>,
    pub annotations: String,
}

static RESISTOR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(\d+(?:\.\d+)?)\s*([kKmMgG]?)\s*([Ωohm]*)(?:\s+(\d+(?:\.\d+)?%?))?$").unwrap()
});
static RESISTOR_R_NOTATION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)([RKM])(\d*)(?:\s+(\d+(?:\.\d+)?%?))?$").unwrap());
static CAPACITOR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(\d+(?:\.\d+)?)\s*([pnuμmµ]?)[fF]?(?:\s+(\d+[Vv]))?(?:\s+(\d+(?:\.\d+)?%?))?$",
    )
    .unwrap()
});
static INDUCTOR_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+(?:\.\d+)?)\s*([pnuμmµ]?)[hH]?$").unwrap());
static WS_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Python `str.strip()`.
fn strip(s: &str) -> &str {
    s.trim()
}

/// Python `float()` of a regex-matched decimal literal.
fn pfloat(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or_else(|_| {
        // Non-ASCII Unicode digits (Python's `\d`) map to their values.
        let ascii: String = s
            .chars()
            .map(|c| {
                c.to_digit(10)
                    .map(|d| char::from(b'0' + d as u8))
                    .unwrap_or(c)
            })
            .collect();
        ascii.parse().unwrap_or(f64::NAN)
    })
}

/// Separate a parseable nominal/tolerance from whitespace annotations.
fn resistor_core(value: &str) -> (String, String) {
    let value = strip(value);
    let mut ends = vec![value.len()];
    let starts: Vec<usize> = WS_RUN.find_iter(value).map(|m| m.start()).collect();
    ends.extend(starts.into_iter().rev());
    for end in ends {
        let core = &value[..end];
        if RESISTOR_R_NOTATION_PATTERN.is_match(core) || RESISTOR_PATTERN.is_match(core) {
            return (core.to_string(), strip(&value[end..]).to_string());
        }
    }
    (value.to_string(), String::new())
}

fn si_multiplier(prefix: &str) -> Option<f64> {
    match prefix {
        "p" => Some(1e-12),
        "n" => Some(1e-9),
        "u" | "μ" | "µ" => Some(1e-6),
        "m" => Some(1e-3),
        _ => None,
    }
}

/// `parse_component_value(value, reference)`.
pub fn parse_component_value(value: &str, reference: &str) -> ParsedValue {
    let value = strip(value).to_string();
    let prefix: String = reference.chars().take(1).collect::<String>().to_uppercase();
    let component_type = match prefix.as_str() {
        "R" => ComponentType::Resistor,
        "C" => ComponentType::Capacitor,
        "L" => ComponentType::Inductor,
        "D" if value.to_uppercase().contains("LED") => ComponentType::Led,
        "D" => ComponentType::Diode,
        "Q" => ComponentType::Transistor,
        "U" => ComponentType::Ic,
        "J" | "P" => ComponentType::Connector,
        "Y" | "X" => ComponentType::Crystal,
        "F" => ComponentType::Fuse,
        _ => ComponentType::Other,
    };
    let mut parsed = ParsedValue {
        raw_value: value.clone(),
        component_type,
        numeric_value: None,
        unit: String::new(),
        tolerance: String::new(),
        voltage_rating: String::new(),
        search_terms: vec![],
        annotations: String::new(),
    };
    let g3 = |x: f64| format_g(x, 3);
    match component_type {
        ComponentType::Resistor => {
            let (core, annotations) = resistor_core(&value);
            parsed.annotations = annotations;
            let mut num: Option<f64> = None;
            let mut tolerance: Option<String> = None;
            if let Some(m) = RESISTOR_R_NOTATION_PATTERN.captures(&core) {
                let whole = &m[1];
                let unit = m[2].to_uppercase();
                let decimal = &m[3];
                let mut n = if decimal.is_empty() {
                    pfloat(whole)
                } else {
                    pfloat(&format!("{whole}.{decimal}"))
                };
                if unit == "K" {
                    n *= 1000.0;
                } else if unit == "M" {
                    n *= 1_000_000.0;
                }
                num = Some(n);
                tolerance = m.get(4).map(|t| t.as_str().to_string());
            } else if let Some(m) = RESISTOR_PATTERN.captures(&core) {
                let mut n = pfloat(&m[1]);
                let g2 = m.get(2).map(|x| x.as_str()).unwrap_or("");
                let mult = g2.to_uppercase();
                let g3s = m.get(3).map(|x| x.as_str()).unwrap_or("");
                if g2 == "m" && !g3s.is_empty() {
                    n *= 1e-3;
                } else if mult == "K" {
                    n *= 1000.0;
                } else if mult == "M" {
                    n *= 1_000_000.0;
                } else if mult == "G" {
                    n *= 1_000_000_000.0;
                }
                num = Some(n);
                tolerance = m.get(4).map(|t| t.as_str().to_string());
            }
            if let Some(n) = num {
                parsed.numeric_value = Some(n);
                parsed.unit = "Ω".into();
                if let Some(t) = tolerance.filter(|t| !t.is_empty()) {
                    parsed.tolerance = t;
                }
                parsed.search_terms.push(if n >= 1_000_000.0 {
                    format!("{}M", g3(n / 1_000_000.0))
                } else if n >= 1000.0 {
                    format!("{}k", g3(n / 1000.0))
                } else {
                    g3(n)
                });
            }
        }
        ComponentType::Capacitor => {
            if let Some(m) = CAPACITOR_PATTERN.captures(&value) {
                let mut n = pfloat(&m[1]);
                let p = m
                    .get(2)
                    .map(|x| x.as_str().to_lowercase())
                    .unwrap_or_default();
                if let Some(k) = si_multiplier(&p) {
                    n *= k;
                }
                parsed.numeric_value = Some(n);
                parsed.unit = "F".into();
                if let Some(v) = m.get(3).filter(|v| !v.as_str().is_empty()) {
                    parsed.voltage_rating = v.as_str().to_string();
                }
                if let Some(t) = m.get(4).filter(|t| !t.as_str().is_empty()) {
                    parsed.tolerance = t.as_str().to_string();
                }
                if n >= 1e-6 {
                    parsed.search_terms.push(format!("{}uF", g3(n * 1e6)));
                } else if n >= 1e-9 {
                    parsed.search_terms.push(format!("{}nF", g3(n * 1e9)));
                } else if n >= 1e-12 {
                    parsed.search_terms.push(format!("{}pF", g3(n * 1e12)));
                }
            }
        }
        ComponentType::Inductor => {
            if let Some(m) = INDUCTOR_PATTERN.captures(&value) {
                let mut n = pfloat(&m[1]);
                let p = m
                    .get(2)
                    .map(|x| x.as_str().to_lowercase())
                    .unwrap_or_default();
                if let Some(k) = si_multiplier(&p) {
                    n *= k;
                }
                parsed.numeric_value = Some(n);
                parsed.unit = "H".into();
                if n >= 1e-6 {
                    parsed.search_terms.push(format!("{}uH", g3(n * 1e6)));
                } else if n >= 1e-9 {
                    parsed.search_terms.push(format!("{}nH", g3(n * 1e9)));
                } else if n >= 1e-12 {
                    parsed.search_terms.push(format!("{}pH", g3(n * 1e12)));
                }
            }
        }
        _ => parsed.search_terms.push(value.clone()),
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        let p = parse_component_value("4K7", "R1");
        assert_eq!(p.numeric_value, Some(4700.0));
        assert_eq!(p.search_terms, vec!["4.7k"]);
        let p = parse_component_value("100nF 25V", "C3");
        assert_eq!(p.voltage_rating, "25V");
        assert!((p.numeric_value.unwrap() - 1e-7).abs() < 1e-20);
        let p = parse_component_value("10k 1% thick film", "R2");
        assert_eq!(p.annotations, "thick film");
        assert_eq!(p.tolerance, "1%");
        assert_eq!(
            parse_component_value("STM32", "U1").search_terms,
            vec!["STM32"]
        );
    }
}
