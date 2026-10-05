//! Port of `kicad_tools.operations.pinmap`: compare symbol pinouts and build
//! pin mappings for symbol replacement.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{bail, Result};
use regex::Regex;

use crate::pyjson::Json;
use crate::sexp::SExp;

/// A symbol pin.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub number: String,
    pub name: String,
    pub pin_type: String,
    pub position: (f64, f64),
    pub orientation: i64,
}

fn unit_suffix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"_\d+$").unwrap())
}

fn unit_variant_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"_\d+_\d+$").unwrap())
}

impl Pin {
    pub fn new(number: &str, name: &str, pin_type: &str) -> Self {
        Pin {
            number: number.into(),
            name: name.into(),
            pin_type: pin_type.into(),
            position: (0.0, 0.0),
            orientation: 0,
        }
    }

    /// Normalized pin name for matching.
    pub fn normalized_name(&self) -> String {
        let name = self.name.to_uppercase();
        let name = unit_suffix_re().replace(&name, "").into_owned();
        name.replace("~{", "")
            .replace('}', "")
            .replace('/', "_")
            .replace('+', "P")
            .replace('-', "N")
    }

    /// Pin function category.
    pub fn function_category(&self) -> &'static str {
        let n = self.name.to_uppercase();
        let any = |pats: &[&str]| pats.iter().any(|p| n.contains(p));
        if any(&["VCC", "VDD", "PVDD", "AVDD", "DVDD", "GVDD", "VBG"]) {
            return "power_positive";
        }
        if any(&["GND", "PGND", "AGND", "EP"]) {
            return "power_ground";
        }
        if n.contains("BST") {
            return "bootstrap";
        }
        if any(&["INPUT", "INP", "INN", "IN_"]) {
            return "audio_input";
        }
        if n.contains("OUT") {
            return "audio_output";
        }
        if any(&["FAULT", "CLIP", "OTW", "SD", "MUTE", "RESET"]) {
            return "status_control";
        }
        if any(&["OSC", "FREQ"]) {
            return "oscillator";
        }
        if any(&["GAIN", "M1", "M2", "HEAD", "PLIMIT", "OC_ADJ"]) {
            return "configuration";
        }
        if ["NC", "N/C", "N.C."].contains(&n.as_str()) {
            return "no_connect";
        }
        "other"
    }
}

/// A source pin and its (optional) target.
#[derive(Debug, Clone, PartialEq)]
pub struct PinMapping {
    pub source_pin: Pin,
    pub target_pin: Option<Pin>,
    pub confidence: f64,
    pub match_reason: String,
}

impl PinMapping {
    pub fn is_matched(&self) -> bool {
        self.target_pin.is_some()
    }
}

/// Complete mapping analysis between two symbols.
#[derive(Debug, Clone, PartialEq)]
pub struct MappingResult {
    pub source_name: String,
    pub target_name: String,
    pub source_pins: Vec<Pin>,
    pub target_pins: Vec<Pin>,
    pub mappings: Vec<PinMapping>,
    pub unmatched_target: Vec<Pin>,
}

impl MappingResult {
    pub fn matched_count(&self) -> usize {
        self.mappings.iter().filter(|m| m.is_matched()).count()
    }

    pub fn unmatched_source_count(&self) -> usize {
        self.mappings.iter().filter(|m| !m.is_matched()).count()
    }

    pub fn match_percentage(&self) -> f64 {
        if self.mappings.is_empty() {
            return 0.0;
        }
        self.matched_count() as f64 / self.mappings.len() as f64 * 100.0
    }

    /// `to_dict()` (upstream key order).
    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("source", self.source_name.as_str());
        d.set("target", self.target_name.as_str());
        d.set("source_pin_count", self.source_pins.len() as i64);
        d.set("target_pin_count", self.target_pins.len() as i64);
        d.set("matched_count", self.matched_count() as i64);
        d.set("match_percentage", self.match_percentage());
        let maps = self
            .mappings
            .iter()
            .map(|m| {
                let mut o = Json::obj();
                o.set("source_number", m.source_pin.number.as_str());
                o.set("source_name", m.source_pin.name.as_str());
                o.set(
                    "target_number",
                    m.target_pin
                        .as_ref()
                        .map_or(Json::Null, |p| Json::Str(p.number.clone())),
                );
                o.set(
                    "target_name",
                    m.target_pin
                        .as_ref()
                        .map_or(Json::Null, |p| Json::Str(p.name.clone())),
                );
                o.set("confidence", m.confidence);
                o.set("reason", m.match_reason.as_str());
                o
            })
            .collect();
        d.set("mappings", Json::Arr(maps));
        let un = self
            .unmatched_target
            .iter()
            .map(|p| {
                let mut o = Json::obj();
                o.set("number", p.number.as_str());
                o.set("name", p.name.as_str());
                o.set("type", p.pin_type.as_str());
                o
            })
            .collect();
        d.set("unmatched_target", Json::Arr(un));
        d
    }
}

fn readable_type(raw: &str) -> String {
    match raw {
        "input" => "Input",
        "output" => "Output",
        "bidirectional" => "Bidirectional",
        "tri_state" => "Tri-State",
        "passive" => "Passive",
        "free" => "Free",
        "unspecified" => "Unspecified",
        "power_in" => "Power Input",
        "power_out" => "Power Output",
        "open_collector" => "Open Collector",
        "open_emitter" => "Open Emitter",
        "no_connect" => "No Connect",
        other => other,
    }
    .to_string()
}

fn string_at(node: &SExp, i: usize) -> Option<String> {
    node.text_at(i)
}

/// Extract pins from a symbol node (deduplicated by number; nested unit
/// symbols searched when `recursive`), sorted numerically then by number.
pub fn extract_pins_from_sexp(symbol: &SExp, recursive: bool) -> Vec<Pin> {
    fn walk(node: &SExp, recursive: bool, seen: &mut BTreeSet<String>, pins: &mut Vec<Pin>) {
        for pin in node.find_all("pin") {
            let raw = string_at(pin, 0)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unspecified".into());
            let name = pin
                .find("name")
                .filter(|n| !n.children.is_empty())
                .and_then(|n| string_at(n, 0))
                .unwrap_or_default();
            let number = pin
                .find("number")
                .filter(|n| !n.children.is_empty())
                .and_then(|n| string_at(n, 0))
                .unwrap_or_default();
            if !seen.insert(number.clone()) {
                continue;
            }
            let (mut position, mut orientation) = ((0.0, 0.0), 0);
            if let Some(at) = pin.find("at").filter(|n| !n.children.is_empty()) {
                position = (at.float_at(0).unwrap_or(0.0), at.float_at(1).unwrap_or(0.0));
                orientation = at.float_at(2).unwrap_or(0.0) as i64;
            }
            pins.push(Pin {
                number,
                name,
                pin_type: readable_type(&raw),
                position,
                orientation,
            });
        }
        if recursive {
            for sub in node.find_all("symbol") {
                walk(sub, recursive, seen, pins);
            }
        }
    }
    let mut pins = Vec::new();
    walk(symbol, recursive, &mut BTreeSet::new(), &mut pins);
    let key = |p: &Pin| {
        let n = if !p.number.is_empty() && p.number.chars().all(|c| c.is_ascii_digit()) {
            p.number.parse::<i64>().unwrap_or(999)
        } else {
            999
        };
        (n, p.number.clone())
    };
    pins.sort_by_key(key);
    pins
}

/// First non-unit-variant symbol of a `.kicad_sym` file.
pub fn load_symbol_from_file(path: &Path) -> Result<(String, Vec<Pin>)> {
    let text = std::fs::read_to_string(path)?;
    let sexp = crate::sexp::parse(&text)?;
    if !sexp.has_tag("kicad_symbol_lib") {
        bail!("Not a symbol library: {}", path.display());
    }
    let symbols: Vec<&SExp> = sexp.find_all("symbol").collect();
    if symbols.is_empty() {
        bail!("No symbols found in: {}", path.display());
    }
    let main = symbols
        .iter()
        .find(|s| !unit_variant_re().is_match(&string_at(s, 0).unwrap_or_default()))
        .copied()
        .unwrap_or(symbols[0]);
    let name = string_at(main, 0)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    Ok((name, extract_pins_from_sexp(main, true)))
}

/// An embedded symbol from a schematic's `lib_symbols`.
pub fn load_symbol_from_schematic(sch_path: &Path, lib_id: &str) -> Result<(String, Vec<Pin>)> {
    let text = std::fs::read_to_string(sch_path)?;
    let sexp = crate::sexp::parse(&text)?;
    if !sexp.has_tag("kicad_sch") {
        bail!("Not a schematic: {}", sch_path.display());
    }
    let Some(lib) = sexp.find("lib_symbols").filter(|l| !l.children.is_empty()) else {
        bail!("No lib_symbols section in: {}", sch_path.display());
    };
    for sym in lib.find_all("symbol") {
        if string_at(sym, 0).as_deref() == Some(lib_id) {
            return Ok((lib_id.to_string(), extract_pins_from_sexp(sym, true)));
        }
    }
    bail!("Symbol '{lib_id}' not found in schematic lib_symbols")
}

/// Match source pins to target pins (exact name, normalized name, same
/// number + category, category suggestion). Returns `(mappings, unmatched)`.
pub fn match_pins(source: &[Pin], target: &[Pin]) -> (Vec<PinMapping>, Vec<Pin>) {
    let mut used: BTreeSet<String> = BTreeSet::new();
    // Python dict comprehensions keep the last pin per key.
    let mut by_name: HashMap<&str, &Pin> = HashMap::new();
    let mut by_number: HashMap<&str, &Pin> = HashMap::new();
    let mut by_norm: HashMap<String, Vec<&Pin>> = HashMap::new();
    let mut by_cat: HashMap<&str, Vec<&Pin>> = HashMap::new();
    for p in target {
        by_name.insert(&p.name, p);
        by_number.insert(&p.number, p);
        by_norm.entry(p.normalized_name()).or_default().push(p);
        by_cat.entry(p.function_category()).or_default().push(p);
    }
    let mut mappings = Vec::new();
    for src in source {
        let mut m: Option<PinMapping> = None;
        if let Some(t) = by_name.get(src.name.as_str()) {
            if !used.contains(&src.name) && !used.contains(&t.number) {
                m = Some(PinMapping {
                    source_pin: src.clone(),
                    target_pin: Some((*t).clone()),
                    confidence: 1.0,
                    match_reason: "Exact name match".into(),
                });
                used.insert(t.number.clone());
            }
        }
        if m.is_none() {
            let norm = src.normalized_name();
            if let Some(list) = by_norm.get(&norm) {
                let cands: Vec<&&Pin> = list.iter().filter(|p| !used.contains(&p.number)).collect();
                if !cands.is_empty() {
                    let t = cands
                        .iter()
                        .find(|p| p.pin_type == src.pin_type)
                        .unwrap_or(&cands[0]);
                    m = Some(PinMapping {
                        source_pin: src.clone(),
                        target_pin: Some((**t).clone()),
                        confidence: 0.8,
                        match_reason: format!("Normalized name match ({norm})"),
                    });
                    used.insert(t.number.clone());
                }
            }
        }
        if m.is_none() {
            if let Some(t) = by_number.get(src.number.as_str()) {
                if !used.contains(&t.number) && src.function_category() == t.function_category() {
                    m = Some(PinMapping {
                        source_pin: src.clone(),
                        target_pin: Some((*t).clone()),
                        confidence: 0.4,
                        match_reason: format!(
                            "Same pin number + category ({})",
                            src.function_category()
                        ),
                    });
                    used.insert(t.number.clone());
                }
            }
        }
        if m.is_none() {
            let cat = src.function_category();
            if cat != "other" {
                if let Some(t) = by_cat
                    .get(cat)
                    .and_then(|l| l.iter().find(|p| !used.contains(&p.number)))
                {
                    m = Some(PinMapping {
                        source_pin: src.clone(),
                        target_pin: Some((*t).clone()),
                        confidence: 0.2,
                        match_reason: format!("Category match ({cat})"),
                    });
                }
            }
        }
        mappings.push(m.unwrap_or_else(|| PinMapping {
            source_pin: src.clone(),
            target_pin: None,
            confidence: 0.0,
            match_reason: "No match found".into(),
        }));
    }
    let unmatched = target
        .iter()
        .filter(|p| !used.contains(&p.number))
        .cloned()
        .collect();
    (mappings, unmatched)
}

fn result(s: (String, Vec<Pin>), t: (String, Vec<Pin>)) -> MappingResult {
    let (mappings, unmatched_target) = match_pins(&s.1, &t.1);
    MappingResult {
        source_name: s.0,
        target_name: t.0,
        source_pins: s.1,
        target_pins: t.1,
        mappings,
        unmatched_target,
    }
}

/// Compare two `.kicad_sym` files.
pub fn compare_symbols(source_path: &Path, target_path: &Path) -> Result<MappingResult> {
    Ok(result(
        load_symbol_from_file(source_path)?,
        load_symbol_from_file(target_path)?,
    ))
}

/// Compare two symbols embedded in a schematic.
pub fn compare_schematic_symbols(
    sch_path: &Path,
    source_lib_id: &str,
    target_lib_id: &str,
) -> Result<MappingResult> {
    Ok(result(
        load_symbol_from_schematic(sch_path, source_lib_id)?,
        load_symbol_from_schematic(sch_path, target_lib_id)?,
    ))
}
