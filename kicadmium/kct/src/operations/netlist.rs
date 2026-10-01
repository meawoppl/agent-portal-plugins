//! Port of `kicad_tools.operations.netlist`: parse KiCad netlists
//! (`kicad-cli sch export netlist --format kicadsexpr`), query them, and
//! walk schematic sheet hierarchies.
//!
//! Gap: upstream's pure-Python fallback extractor
//! (`build_netlist_from_schematic`) is built on `kicad_tools.schematic.models`
//! connectivity, which is not ported; here the fallback reports an error
//! instead of extracting.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Result};

use crate::pyjson::{py_round, Json};
use crate::sexp::SExp;

pub use crate::cli::runner::find_kicad_cli;

fn gs(node: &SExp, i: usize) -> String {
    node.text_at(i).unwrap_or_default()
}

fn child<'a>(node: &'a SExp, tag: &str) -> Option<&'a SExp> {
    node.find(tag).filter(|n| !n.children.is_empty())
}

fn child_str(node: &SExp, tag: &str) -> String {
    child(node, tag).map(|n| gs(n, 0)).unwrap_or_default()
}

fn child_int(node: &SExp, tag: &str) -> i64 {
    child(node, tag)
        .and_then(|n| n.value_at(0).and_then(|v| v.as_i64()))
        .unwrap_or(0)
}

/// A pin on a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentPin {
    pub number: String,
    pub name: String,
    pub pin_type: String,
}

/// A component in the netlist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetlistComponent {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub lib_id: String,
    pub sheet_path: String,
    /// Insertion-ordered `(name, value)` pairs (Python dict semantics).
    pub properties: Vec<(String, String)>,
    pub pins: Vec<ComponentPin>,
}

impl NetlistComponent {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut lib_id = String::new();
        if let Some(lib) = child(sexp, "libsource") {
            if let Some(part) = child(lib, "part") {
                lib_id = gs(part, 0);
            } else if !gs(lib, 1).is_empty() {
                lib_id = gs(lib, 1);
            }
        }
        let sheet_path = child(sexp, "sheetpath")
            .and_then(|s| child(s, "names"))
            .map(|n| gs(n, 0))
            .unwrap_or_default();
        // KiCad writes `(property (name "K") (value "V"))`. Upstream reads
        // positional strings, which stringifies those child nodes (a bug);
        // here the named children are read, with positional atoms as the
        // fallback for `(property "K" "V")`.
        let mut properties: Vec<(String, String)> = Vec::new();
        for prop in sexp.find_all("property") {
            let (k, v) = match (child(prop, "name"), child(prop, "value")) {
                (Some(n), Some(v)) => (gs(n, 0), gs(v, 0)),
                _ => (
                    prop.string_at(0).unwrap_or_default().to_string(),
                    prop.string_at(1).unwrap_or_default().to_string(),
                ),
            };
            if !k.is_empty() && !v.is_empty() {
                if let Some(slot) = properties.iter_mut().find(|(n, _)| *n == k) {
                    slot.1 = v;
                } else {
                    properties.push((k, v));
                }
            }
        }
        NetlistComponent {
            reference: child_str(sexp, "ref"),
            value: child_str(sexp, "value"),
            footprint: child_str(sexp, "footprint"),
            lib_id,
            sheet_path,
            properties,
            pins: Vec::new(),
        }
    }
}

/// A connection point in a net.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetNode {
    pub reference: String,
    pub pin: String,
    pub pin_function: String,
    pub pin_type: String,
}

impl NetNode {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let reference = match child(sexp, "ref") {
            Some(r) => gs(r, 0),
            None => gs(sexp, 0),
        };
        NetNode {
            reference,
            pin: child_str(sexp, "pin"),
            pin_function: child_str(sexp, "pinfunction"),
            pin_type: child_str(sexp, "pintype"),
        }
    }
}

/// A net in the netlist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetlistNet {
    pub code: i64,
    pub name: String,
    pub nodes: Vec<NetNode>,
}

impl NetlistNet {
    pub fn from_sexp(sexp: &SExp) -> Self {
        NetlistNet {
            code: child_int(sexp, "code"),
            name: child_str(sexp, "name"),
            nodes: sexp.find_all("node").map(NetNode::from_sexp).collect(),
        }
    }

    pub fn connection_count(&self) -> usize {
        self.nodes.len()
    }
}

/// A schematic sheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SheetInfo {
    pub number: i64,
    pub name: String,
    pub path: String,
    pub title: String,
    pub source: String,
}

/// Parsed netlist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Netlist {
    pub source_file: String,
    pub tool: String,
    pub date: String,
    pub sheets: Vec<SheetInfo>,
    pub components: Vec<NetlistComponent>,
    pub nets: Vec<NetlistNet>,
}

impl Netlist {
    pub fn from_sexp(sexp: &SExp) -> Result<Self> {
        if !sexp.has_tag("export") {
            bail!(
                "Expected 'export' root, got '{}'",
                sexp.tag().unwrap_or("None")
            );
        }
        let mut nl = Netlist::default();
        if let Some(design) = child(sexp, "design") {
            nl.source_file = child_str(design, "source");
            nl.tool = child_str(design, "tool");
            nl.date = child_str(design, "date");
            for sheet in design.find_all("sheet") {
                let (mut title, mut source) = (String::new(), String::new());
                if let Some(tb) = child(sheet, "title_block") {
                    title = child_str(tb, "title");
                    source = child_str(tb, "source");
                }
                nl.sheets.push(SheetInfo {
                    number: child_int(sheet, "number"),
                    name: child_str(sheet, "name"),
                    path: child_str(sheet, "tstamps"),
                    title,
                    source,
                });
            }
        }
        if let Some(comps) = child(sexp, "components") {
            nl.components = comps
                .find_all("comp")
                .map(NetlistComponent::from_sexp)
                .collect();
        }
        if let Some(nets) = child(sexp, "nets") {
            nl.nets = nets.find_all("net").map(NetlistNet::from_sexp).collect();
        }
        Ok(nl)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())?;
        Self::from_sexp(&crate::sexp::parse(&text)?)
    }

    pub fn get_component(&self, reference: &str) -> Option<&NetlistComponent> {
        self.components.iter().find(|c| c.reference == reference)
    }

    pub fn get_net(&self, name: &str) -> Option<&NetlistNet> {
        self.nets.iter().find(|n| n.name == name)
    }

    pub fn get_component_nets(&self, reference: &str) -> Vec<&NetlistNet> {
        self.nets
            .iter()
            .filter(|n| n.nodes.iter().any(|x| x.reference == reference))
            .collect()
    }

    pub fn get_net_by_pin(&self, reference: &str, pin: &str) -> Option<&NetlistNet> {
        self.nets.iter().find(|n| {
            n.nodes
                .iter()
                .any(|x| x.reference == reference && x.pin == pin)
        })
    }

    /// Nets containing a pin whose type mentions `power`.
    pub fn power_nets(&self) -> Vec<&NetlistNet> {
        self.nets
            .iter()
            .filter(|n| {
                n.nodes
                    .iter()
                    .any(|x| x.pin_type.to_lowercase().contains("power"))
            })
            .collect()
    }

    pub fn find_single_pin_nets(&self) -> Vec<&NetlistNet> {
        self.nets
            .iter()
            .filter(|n| n.connection_count() == 1)
            .collect()
    }

    /// `(reference, pin, net_name)` of pins alone on their net.
    pub fn find_floating_pins(&self) -> Vec<(String, String, String)> {
        self.nets
            .iter()
            .filter(|n| n.nodes.len() == 1)
            .map(|n| {
                (
                    n.nodes[0].reference.clone(),
                    n.nodes[0].pin.clone(),
                    n.name.clone(),
                )
            })
            .collect()
    }

    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("source", self.source_file.as_str());
        d.set("tool", self.tool.as_str());
        d.set("date", self.date.as_str());
        d.set(
            "sheets",
            Json::Arr(
                self.sheets
                    .iter()
                    .map(|s| {
                        let mut o = Json::obj();
                        o.set("number", s.number);
                        o.set("name", s.name.as_str());
                        o.set("path", s.path.as_str());
                        o.set("title", s.title.as_str());
                        o.set("source", s.source.as_str());
                        o
                    })
                    .collect(),
            ),
        );
        d.set(
            "components",
            Json::Arr(
                self.components
                    .iter()
                    .map(|c| {
                        let mut o = Json::obj();
                        o.set("reference", c.reference.as_str());
                        o.set("value", c.value.as_str());
                        o.set("footprint", c.footprint.as_str());
                        o.set("lib_id", c.lib_id.as_str());
                        o.set("sheet_path", c.sheet_path.as_str());
                        let mut props = Json::obj();
                        for (k, v) in &c.properties {
                            props.set(k, v.as_str());
                        }
                        o.set("properties", props);
                        o
                    })
                    .collect(),
            ),
        );
        d.set(
            "nets",
            Json::Arr(
                self.nets
                    .iter()
                    .map(|n| {
                        let mut o = Json::obj();
                        o.set("code", n.code);
                        o.set("name", n.name.as_str());
                        o.set("connections", n.connection_count() as i64);
                        o.set(
                            "nodes",
                            Json::Arr(
                                n.nodes
                                    .iter()
                                    .map(|x| {
                                        let mut m = Json::obj();
                                        m.set("reference", x.reference.as_str());
                                        m.set("pin", x.pin.as_str());
                                        m.set("pin_function", x.pin_function.as_str());
                                        m.set("pin_type", x.pin_type.as_str());
                                        m
                                    })
                                    .collect(),
                            ),
                        );
                        o
                    })
                    .collect(),
            ),
        );
        d
    }

    /// `json.dumps(to_dict(), indent=indent)`.
    pub fn to_json(&self, indent: usize) -> String {
        crate::pyjson::dumps_indent(&self.to_dict(), indent)
    }

    /// Summary statistics.
    pub fn summary(&self) -> Json {
        let mut by_prefix: Vec<(String, i64)> = Vec::new();
        for c in &self.components {
            let prefix: String = c
                .reference
                .chars()
                .filter(|ch| ch.is_alphabetic())
                .collect();
            match by_prefix.iter_mut().find(|(p, _)| *p == prefix) {
                Some(slot) => slot.1 += 1,
                None => by_prefix.push((prefix, 1)),
            }
        }
        const POWER: [&str; 6] = ["GND", "PGND", "AGND", "VCC", "VDD", "VBUS"];
        let power = self
            .nets
            .iter()
            .filter(|n| n.name.starts_with('+') || POWER.contains(&n.name.as_str()))
            .count() as i64;
        let mut d = Json::obj();
        d.set("source_file", self.source_file.as_str());
        d.set("tool", self.tool.as_str());
        d.set("date", self.date.as_str());
        d.set("sheet_count", self.sheets.len() as i64);
        d.set("component_count", self.components.len() as i64);
        let mut types = Json::obj();
        for (p, n) in by_prefix {
            types.set(&p, n);
        }
        d.set("components_by_type", types);
        d.set("net_count", self.nets.len() as i64);
        d.set("power_net_count", power);
        d.set("signal_net_count", self.nets.len() as i64 - power);
        d
    }
}

/// A sub-sheet entry: file, `Sheetname`, sheet pins, and symbol UUID.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SheetEntry {
    pub filename: String,
    pub sheetname: String,
    pub pin_names: Vec<String>,
    pub pin_positions: Vec<(f64, f64)>,
    pub uuid: String,
}

/// `_get_sheet_entries`: top-level `(sheet ...)` entries with a `Sheetfile`.
pub fn get_sheet_entries(sch_path: &Path) -> Result<Vec<SheetEntry>> {
    let doc = crate::sexp::parse(&std::fs::read_to_string(sch_path)?)?;
    let mut out = Vec::new();
    for sheet in doc.children.iter().filter(|c| c.has_tag("sheet")) {
        let uuid = child(sheet, "uuid").map(|u| gs(u, 0)).unwrap_or_default();
        let (mut sx, mut sy) = (0.0, 0.0);
        if let Some(at) = child(sheet, "at") {
            let atoms: Vec<_> = at.atoms().collect();
            if atoms.len() >= 2 {
                sx = py_round(atoms[0].as_f64().unwrap_or(0.0), 2);
                sy = py_round(atoms[1].as_f64().unwrap_or(0.0), 2);
            }
        }
        let (mut filename, mut sheetname) = (String::new(), String::new());
        for prop in sheet.find_all("property") {
            match gs(prop, 0).as_str() {
                "Sheetfile" if !gs(prop, 1).is_empty() => filename = gs(prop, 1),
                "Sheetname" if !gs(prop, 1).is_empty() => sheetname = gs(prop, 1),
                _ => {}
            }
        }
        let (mut pin_names, mut pin_positions) = (Vec::new(), Vec::new());
        for pin in sheet.find_all("pin") {
            let name = gs(pin, 0);
            if name.is_empty() {
                continue;
            }
            pin_names.push(name);
            let pos = child(pin, "at")
                .and_then(|at| {
                    let atoms: Vec<_> = at.atoms().collect();
                    (atoms.len() >= 2).then(|| {
                        (
                            py_round(atoms[0].as_f64().unwrap_or(0.0), 2),
                            py_round(atoms[1].as_f64().unwrap_or(0.0), 2),
                        )
                    })
                })
                .unwrap_or((sx, sy));
            pin_positions.push(pos);
        }
        if !filename.is_empty() {
            out.push(SheetEntry {
                filename,
                sheetname,
                pin_names,
                pin_positions,
                uuid,
            });
        }
    }
    Ok(out)
}

/// `_get_sheet_filenames`.
pub fn get_sheet_filenames(sch_path: &Path) -> Result<Vec<String>> {
    Ok(get_sheet_entries(sch_path)?
        .into_iter()
        .map(|e| e.filename)
        .collect())
}

/// `_count_hierarchy_sheets`: total sheets including the root; cycles and
/// missing files contribute nothing.
pub fn count_hierarchy_sheets(sch_path: &Path) -> usize {
    fn walk(p: &Path, visited: &mut BTreeSet<PathBuf>) -> usize {
        let resolved = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if !visited.insert(resolved) || !p.exists() {
            return 0;
        }
        let mut n = 1;
        let parent = p.parent().unwrap_or(Path::new("."));
        for f in get_sheet_filenames(p).unwrap_or_default() {
            n += walk(&parent.join(f), visited);
        }
        n
    }
    walk(sch_path, &mut BTreeSet::new())
}

/// Upstream's pure-Python extractor (not ported; see module docs).
pub fn build_netlist_from_schematic(sch_path: &Path) -> Result<Netlist> {
    if !sch_path.exists() {
        bail!("Schematic not found: {}", sch_path.display());
    }
    bail!(
        "native schematic netlist extraction is not available; kicad-cli is required to export a netlist for {}",
        sch_path.display()
    )
}

/// Export a netlist with `kicad-cli` and parse it. `output_path` defaults
/// to `<dir>/<stem>-netlist.kicad_net` beside the schematic. With
/// `fallback`, failures fall through to [`build_netlist_from_schematic`].
pub fn export_netlist(
    sch_path: &Path,
    output_path: Option<&Path>,
    kicad_cli: Option<&Path>,
    format: &str,
    fallback: bool,
) -> Result<Netlist> {
    if !sch_path.exists() {
        bail!("Schematic not found: {}", sch_path.display());
    }
    let cli = match kicad_cli {
        Some(p) => p.to_path_buf(),
        None => match find_kicad_cli() {
            Some(p) => p,
            None if fallback => {
                eprintln!("WARNING: kicad-cli not found, using native netlist extraction");
                return build_netlist_from_schematic(sch_path);
            }
            None => bail!("kicad-cli not found. Install KiCad 8."),
        },
    };
    let output = match output_path {
        Some(p) => p.to_path_buf(),
        None => {
            let stem = sch_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            sch_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(format!("{stem}-netlist.kicad_net"))
        }
    };
    if output.exists() {
        std::fs::remove_file(&output)?;
    }
    let result = Command::new(&cli)
        .args(["sch", "export", "netlist", "--format", format, "--output"])
        .arg(&output)
        .arg(sch_path)
        .output();
    let result = match result {
        Ok(r) => r,
        Err(e) if fallback => {
            eprintln!("WARNING: kicad-cli subprocess error ({e}), using native netlist extraction");
            return build_netlist_from_schematic(sch_path);
        }
        Err(e) => bail!("Netlist export failed: {e}"),
    };
    let code = result.status.code().unwrap_or(-1);
    #[cfg(unix)]
    let crashed = {
        use std::os::unix::process::ExitStatusExt;
        code == 139 || result.status.signal() == Some(11)
    };
    #[cfg(not(unix))]
    let crashed = code == 139;
    if crashed {
        if fallback {
            return build_netlist_from_schematic(sch_path);
        }
        bail!(
            "kicad-cli crashed (SIGSEGV). This may be caused by a problematic symbol in the schematic. Try removing recently added symbols or exporting the netlist manually from KiCad GUI. See: https://gitlab.com/kicad/code/kicad/-/issues"
        );
    }
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            format!("Exit code {code}")
        } else {
            stderr
        };
        if fallback {
            eprintln!("WARNING: kicad-cli failed ({msg}), using native netlist extraction");
            return build_netlist_from_schematic(sch_path);
        }
        bail!("kicad-cli failed: {msg}");
    }
    if !output.exists() {
        if fallback {
            return build_netlist_from_schematic(sch_path);
        }
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                "Netlist export produced no output".to_string()
            } else {
                stderr
            }
        );
    }
    let netlist = Netlist::load(&output)?;
    if fallback {
        let expected = count_hierarchy_sheets(sch_path);
        let exported = netlist.sheets.len().max(1);
        if exported < expected {
            eprintln!(
                "WARNING: kicad-cli netlist is incomplete: exported {exported} of {expected} sheets"
            );
            return build_netlist_from_schematic(sch_path).or(Ok(netlist));
        }
    }
    Ok(netlist)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: &str = r#"(export (version "E")
  (design (source "/x/a.kicad_sch") (tool "Eeschema 8.0.0") (date "today")
    (sheet (number "1") (name "/") (tstamps "/")
      (title_block (title "T") (source "a.kicad_sch"))))
  (components
    (comp (ref "R1") (value "10k") (footprint "R:0603")
      (libsource (lib "Device") (part "R") (description ""))
      (property (name "Sim") (value "x"))
      (sheetpath (names "/") (tstamps "/")))
    (comp (ref "U1") (value "MCU") (libsource (lib "M") (part "MCU"))))
  (nets
    (net (code "1") (name "GND") (node (ref "R1") (pin "2") (pintype "passive")) (node (ref "U1") (pin "8") (pintype "power_in")))
    (net (code "2") (name "Net-(R1-Pad1)") (node (ref "R1") (pin "1") (pinfunction "A") (pintype "passive")))))"#;

    #[test]
    fn parses_and_queries() {
        let nl = Netlist::from_sexp(&crate::sexp::parse(NET).unwrap()).unwrap();
        assert_eq!(nl.tool, "Eeschema 8.0.0");
        assert_eq!(nl.sheets[0].title, "T");
        assert_eq!(nl.components[0].lib_id, "R");
        assert_eq!(nl.components[0].sheet_path, "/");
        assert_eq!(nl.nets[0].code, 1);
        assert_eq!(nl.get_net_by_pin("U1", "8").unwrap().name, "GND");
        assert_eq!(nl.power_nets().len(), 1);
        assert_eq!(
            nl.find_floating_pins(),
            vec![("R1".into(), "1".into(), "Net-(R1-Pad1)".into())]
        );
        assert_eq!(nl.get_component_nets("R1").len(), 2);
        let s = crate::pyjson::dumps(&nl.summary());
        assert!(
            s.contains("\"components_by_type\": {\"R\": 1, \"U\": 1}"),
            "{s}"
        );
        assert!(s.contains("\"power_net_count\": 1"), "{s}");
    }
}
