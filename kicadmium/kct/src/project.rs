//! KiCad project handling (port of `kicad_tools.project`).
//!
//! [`Project`] ties a `.kicad_pro` to its schematic and PCB: path
//! resolution (`load` / `from_pcb` / `create`), lazy s-expression loading,
//! and schematic-vs-PCB cross-referencing. Upstream's high-level wrappers
//! over the router, DRC, BOM, and exporters (`route`, `check_drc`,
//! `get_bom`, `export_*`) live with those subsystems' ports.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{json, Value};

use crate::core::version::{
    KICAD_BOARD_FORMAT_VERSION, KICAD_GENERATOR_VERSION, KICAD_SCH_FORMAT_VERSION,
};
use crate::sexp::{parse, SExp};
use crate::utils::pyrepr::{py_float_repr, py_str_repr};

/// Result of a routing operation.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingResult {
    pub routed_nets: usize,
    pub total_nets: usize,
    pub total_segments: usize,
    pub total_vias: usize,
    pub total_length_mm: f64,
}

impl RoutingResult {
    /// Fraction of nets routed (1.0 when there are none).
    pub fn success_rate(&self) -> f64 {
        if self.total_nets == 0 {
            1.0
        } else {
            self.routed_nets as f64 / self.total_nets as f64
        }
    }
}

/// A schematic symbol with no corresponding PCB footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct UnplacedSymbol {
    pub reference: String,
    pub value: String,
    pub lib_id: String,
    /// Expected footprint.
    pub footprint_name: String,
}

/// A PCB footprint with no corresponding schematic symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct OrphanedFootprint {
    pub reference: String,
    pub value: String,
    pub footprint_name: String,
    pub position: (f64, f64),
}

/// A component whose schematic and PCB data disagree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MismatchedComponent {
    pub reference: String,
    pub schematic_value: String,
    pub pcb_value: String,
    pub schematic_footprint: String,
    pub pcb_footprint: String,
    /// `"value"` and/or `"footprint"`.
    pub mismatches: Vec<String>,
}

/// Result of cross-referencing schematic and PCB.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CrossReferenceResult {
    pub matched: usize,
    pub unplaced: Vec<UnplacedSymbol>,
    pub orphaned: Vec<OrphanedFootprint>,
    pub mismatched: Vec<MismatchedComponent>,
}

impl CrossReferenceResult {
    pub fn is_clean(&self) -> bool {
        self.unplaced.is_empty() && self.orphaned.is_empty() && self.mismatched.is_empty()
    }

    /// `{"matched", "unplaced", "orphaned", "mismatched"}` counts.
    pub fn summary(&self) -> Value {
        json!({
            "matched": self.matched,
            "unplaced": self.unplaced.len(),
            "orphaned": self.orphaned.len(),
            "mismatched": self.mismatched.len(),
        })
    }
}

/// Schematic-side component data for [`cross_reference_components`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchematicComponent {
    pub reference: String,
    pub value: String,
    pub lib_id: String,
    pub footprint: String,
}

/// PCB-side component data for [`cross_reference_components`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PcbComponent {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub position: (f64, f64),
}

/// Placed symbols (`(symbol (lib_id ...))` at the schematic root).
pub fn schematic_components(root: &SExp) -> Vec<SchematicComponent> {
    root.children_named("symbol")
        .filter(|s| s.get("lib_id").is_some())
        .map(|s| SchematicComponent {
            reference: s.property("Reference").unwrap_or("").to_string(),
            value: s.property("Value").unwrap_or("").to_string(),
            lib_id: s.child_str("lib_id").unwrap_or("").to_string(),
            footprint: s.property("Footprint").unwrap_or("").to_string(),
        })
        .collect()
}

fn fp_text(node: &SExp, kind: &str) -> Option<String> {
    node.children_named("fp_text")
        .find(|t| t.string_at(0) == Some(kind))
        .and_then(|t| t.text_at(1))
}

/// Footprints (`footprint` / legacy `module`) at the board root.
pub fn pcb_components(root: &SExp) -> Vec<PcbComponent> {
    root.children
        .iter()
        .filter(|c| c.has_tag("footprint") || c.has_tag("module"))
        .map(|fp| {
            let (x, y, _) = fp.at().unwrap_or((0.0, 0.0, 0.0));
            PcbComponent {
                reference: fp
                    .property("Reference")
                    .map(str::to_string)
                    .or_else(|| fp_text(fp, "reference"))
                    .unwrap_or_default(),
                value: fp
                    .property("Value")
                    .map(str::to_string)
                    .or_else(|| fp_text(fp, "value"))
                    .unwrap_or_default(),
                footprint: fp.text_at(0).unwrap_or_default(),
                position: (x, y),
            }
        })
        .collect()
}

/// Cross-reference by reference designator (references starting with `#`,
/// e.g. power symbols, and empty references are ignored). Footprints are
/// compared without their library prefix. Results are sorted by reference.
pub fn cross_reference_components(
    schematic: &[SchematicComponent],
    pcb: &[PcbComponent],
) -> CrossReferenceResult {
    let keep = |r: &str| !r.is_empty() && !r.starts_with('#');
    let sch: BTreeMap<&str, &SchematicComponent> = schematic
        .iter()
        .filter(|c| keep(&c.reference))
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let brd: BTreeMap<&str, &PcbComponent> = pcb
        .iter()
        .filter(|c| keep(&c.reference))
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let short = |f: &str| f.rsplit(':').next().unwrap_or(f).to_string();
    let mut result = CrossReferenceResult::default();
    for (reference, s) in &sch {
        match brd.get(reference) {
            Some(p) => {
                result.matched += 1;
                let mut mismatches = Vec::new();
                if s.value != p.value {
                    mismatches.push("value".to_string());
                }
                if !s.footprint.is_empty()
                    && !p.footprint.is_empty()
                    && short(&s.footprint) != short(&p.footprint)
                {
                    mismatches.push("footprint".to_string());
                }
                if !mismatches.is_empty() {
                    result.mismatched.push(MismatchedComponent {
                        reference: reference.to_string(),
                        schematic_value: s.value.clone(),
                        pcb_value: p.value.clone(),
                        schematic_footprint: s.footprint.clone(),
                        pcb_footprint: p.footprint.clone(),
                        mismatches,
                    });
                }
            }
            None => result.unplaced.push(UnplacedSymbol {
                reference: reference.to_string(),
                value: s.value.clone(),
                lib_id: s.lib_id.clone(),
                footprint_name: s.footprint.clone(),
            }),
        }
    }
    for (reference, p) in &brd {
        if !sch.contains_key(reference) {
            result.orphaned.push(OrphanedFootprint {
                reference: reference.to_string(),
                value: p.value.clone(),
                footprint_name: p.footprint.clone(),
                position: p.position,
            });
        }
    }
    result
}

/// High-level handle on a KiCad project's files.
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub project_file: Option<PathBuf>,
    pub schematic_path: Option<PathBuf>,
    pub pcb_path: Option<PathBuf>,
    /// Warnings raised while resolving files (also printed to stderr).
    pub warnings: Vec<String>,
    schematic: Option<SExp>,
    pcb: Option<SExp>,
}

fn not_found(message: String) -> anyhow::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, message).into()
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Project {
    pub fn new(
        schematic: Option<impl AsRef<Path>>,
        pcb: Option<impl AsRef<Path>>,
        project_file: Option<impl AsRef<Path>>,
    ) -> Self {
        Self {
            project_file: project_file.map(|p| p.as_ref().to_path_buf()),
            schematic_path: schematic.map(|p| p.as_ref().to_path_buf()),
            pcb_path: pcb.map(|p| p.as_ref().to_path_buf()),
            ..Default::default()
        }
    }

    fn warn(&mut self, message: String) {
        eprintln!("{message}");
        self.warnings.push(message);
    }

    /// Load from a `.kicad_pro`. PCB resolution: a non-empty `boards[]`
    /// array's first existing `.kicad_pcb` entry; else `<name>.kicad_pcb`;
    /// else a single sibling `*.kicad_pcb` (with a warning); several
    /// siblings is an error; none leaves the PCB unset (with a warning).
    /// Errors are `std::io::Error` with kind `NotFound`.
    pub fn load(project_path: impl AsRef<Path>) -> crate::Result<Project> {
        let project_path = project_path.as_ref();
        if !project_path.exists() {
            return Err(not_found(format!(
                "Project file not found: {}",
                project_path.display()
            )));
        }
        let project_dir = project_path.parent().unwrap_or(Path::new("")).to_path_buf();
        let project_name = stem(project_path);
        let mut warnings = Vec::new();

        let mut pcb_from_pro: Option<PathBuf> = None;
        match std::fs::read_to_string(project_path) {
            Ok(text) => {
                let parsed: Result<Value, _> = if text.trim().is_empty() {
                    Ok(json!({}))
                } else {
                    serde_json::from_str(&text)
                };
                match parsed {
                    Ok(data) => {
                        if let Some(boards) = data.get("boards").and_then(Value::as_array) {
                            for entry in boards.iter().filter_map(Value::as_object) {
                                let file = entry
                                    .get("file")
                                    .and_then(Value::as_str)
                                    .filter(|s| !s.is_empty())
                                    .or_else(|| entry.get("filename").and_then(Value::as_str))
                                    .filter(|s| !s.is_empty());
                                let Some(file) = file else { continue };
                                let joined = project_dir.join(file);
                                let candidate = std::fs::canonicalize(&joined).unwrap_or(joined);
                                if candidate.exists()
                                    && candidate.extension().is_some_and(|e| e == "kicad_pcb")
                                {
                                    pcb_from_pro = Some(candidate);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => warnings.push(format!(
                        "Could not parse {} as JSON ({e}); falling back to basename convention.",
                        project_path.display()
                    )),
                }
            }
            Err(e) => warnings.push(format!(
                "Could not parse {} as JSON ({e}); falling back to basename convention.",
                project_path.display()
            )),
        }

        let schematic_path = project_dir.join(format!("{project_name}.kicad_sch"));
        let mut pcb_path =
            pcb_from_pro.unwrap_or_else(|| project_dir.join(format!("{project_name}.kicad_pcb")));
        if !pcb_path.exists() {
            let mut siblings: Vec<PathBuf> =
                std::fs::read_dir(if project_dir.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &project_dir
                })
                .map(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().is_some_and(|e| e == "kicad_pcb"))
                        .collect()
                })
                .unwrap_or_default();
            siblings.sort();
            match siblings.len() {
                0 => warnings.push(format!(
                    "No .kicad_pcb files found in {} (canonical {} missing). Project loaded \
                     without a PCB path.",
                    project_dir.display(),
                    file_name(&pcb_path)
                )),
                1 => {
                    warnings.push(format!(
                        "Canonical PCB {} not found in {}; using {} instead. Pass --pcb \
                         explicitly to silence this warning.",
                        file_name(&pcb_path),
                        project_dir.display(),
                        file_name(&siblings[0])
                    ));
                    pcb_path = project_dir.join(file_name(&siblings[0]));
                }
                _ => {
                    let names: Vec<String> = siblings.iter().map(|p| file_name(p)).collect();
                    return Err(not_found(format!(
                        "Project {} loaded, but canonical {} not found and multiple .kicad_pcb \
                         candidates exist in {}: {}. Pass --pcb explicitly to disambiguate.",
                        file_name(project_path),
                        file_name(&pcb_path),
                        project_dir.display(),
                        names.join(", ")
                    )));
                }
            }
        }

        let mut project = Project::new(
            schematic_path.exists().then_some(schematic_path),
            pcb_path.exists().then_some(pcb_path),
            Some(project_path),
        );
        for w in warnings {
            project.warn(w);
        }
        Ok(project)
    }

    /// Create `<name>.kicad_pro`, `.kicad_sch`, and `.kicad_pcb` (with a
    /// `board_width` x `board_height` mm Edge.Cuts rectangle) in `directory`.
    pub fn create(
        name: &str,
        directory: impl AsRef<Path>,
        board_width: f64,
        board_height: f64,
    ) -> crate::Result<Project> {
        let directory = directory.as_ref();
        std::fs::create_dir_all(directory)
            .with_context(|| format!("creating {}", directory.display()))?;
        let project_path = directory.join(format!("{name}.kicad_pro"));
        let schematic_path = directory.join(format!("{name}.kicad_sch"));
        let pcb_path = directory.join(format!("{name}.kicad_pcb"));
        let uuid = || uuid::Uuid::new_v4().to_string();

        let project_data = json!({
            "meta": {"filename": format!("{name}.kicad_pro"), "version": 1},
            "project": {"uuid": uuid()},
        });
        std::fs::write(&project_path, serde_json::to_string_pretty(&project_data)?)?;

        let schematic = format!(
            "(kicad_sch (version {KICAD_SCH_FORMAT_VERSION}) (generator \"kicad_tools\") \
             (generator_version \"{KICAD_GENERATOR_VERSION}\")\n\n  (uuid \"{}\")\n\n  \
             (paper \"A4\")\n\n  (lib_symbols\n  )\n\n  (symbol_instances\n  )\n)\n",
            uuid()
        );
        std::fs::write(&schematic_path, schematic)?;

        let pcb = PCB_TEMPLATE
            .replace("{VERSION}", &KICAD_BOARD_FORMAT_VERSION.to_string())
            .replace("{GENERATOR_VERSION}", KICAD_GENERATOR_VERSION)
            .replace("{PCB_UUID}", &uuid())
            .replace("{WIDTH}", &py_float_repr(board_width))
            .replace("{HEIGHT}", &py_float_repr(board_height))
            .replace("{RECT_UUID}", &uuid());
        std::fs::write(&pcb_path, pcb)?;

        Ok(Project::new(
            Some(schematic_path),
            Some(pcb_path),
            Some(project_path),
        ))
    }

    /// Project around a PCB, picking up sibling `.kicad_sch` / `.kicad_pro`.
    pub fn from_pcb(pcb_path: impl AsRef<Path>) -> Project {
        let pcb_path = pcb_path.as_ref();
        let schematic = pcb_path.with_extension("kicad_sch");
        let project = pcb_path.with_extension("kicad_pro");
        Project::new(
            schematic.exists().then_some(schematic),
            Some(pcb_path),
            project.exists().then_some(project),
        )
    }

    /// Project name: project file, else PCB, else schematic stem, else
    /// `"unnamed"`.
    pub fn name(&self) -> String {
        [&self.project_file, &self.pcb_path, &self.schematic_path]
            .into_iter()
            .flatten()
            .next()
            .map(|p| stem(p))
            .unwrap_or_else(|| "unnamed".to_string())
    }

    /// Directory of the project file, else PCB, else schematic.
    pub fn directory(&self) -> Option<PathBuf> {
        [&self.project_file, &self.pcb_path, &self.schematic_path]
            .into_iter()
            .flatten()
            .next()
            .map(|p| p.parent().unwrap_or(Path::new("")).to_path_buf())
    }

    /// Lazily parsed schematic tree (`None` without an existing file).
    pub fn schematic(&mut self) -> crate::Result<Option<&SExp>> {
        if self.schematic.is_none() {
            if let Some(path) = self.schematic_path.as_ref().filter(|p| p.exists()) {
                let text = std::fs::read_to_string(path)?;
                self.schematic = Some(parse(&text)?);
            }
        }
        Ok(self.schematic.as_ref())
    }

    /// Lazily parsed PCB tree (`None` without an existing file).
    pub fn pcb(&mut self) -> crate::Result<Option<&SExp>> {
        if self.pcb.is_none() {
            if let Some(path) = self.pcb_path.as_ref().filter(|p| p.exists()) {
                let text = std::fs::read_to_string(path)?;
                self.pcb = Some(parse(&text)?);
            }
        }
        Ok(self.pcb.as_ref())
    }

    /// Cross-reference schematic symbols with PCB footprints (empty result,
    /// with a warning, when either side is missing).
    pub fn cross_reference(&mut self) -> crate::Result<CrossReferenceResult> {
        let sch = self.schematic()?.map(schematic_components);
        let pcb = self.pcb()?.map(pcb_components);
        match (sch, pcb) {
            (Some(s), Some(p)) => Ok(cross_reference_components(&s, &p)),
            _ => {
                self.warn("Cannot cross-reference: missing schematic or PCB".to_string());
                Ok(CrossReferenceResult::default())
            }
        }
    }

    pub fn find_unplaced_symbols(&mut self) -> crate::Result<Vec<UnplacedSymbol>> {
        Ok(self.cross_reference()?.unplaced)
    }

    pub fn find_orphaned_footprints(&mut self) -> crate::Result<Vec<OrphanedFootprint>> {
        Ok(self.cross_reference()?.orphaned)
    }

    /// Candidate DRC report paths (`<name>-drc.rpt`, `<name>_drc.rpt`,
    /// `drc.rpt`), first existing wins (used by `check_drc`).
    pub fn find_drc_report(&self) -> Option<PathBuf> {
        let dir = self.directory()?;
        let name = self.name();
        [
            format!("{name}-drc.rpt"),
            format!("{name}_drc.rpt"),
            "drc.rpt".to_string(),
        ]
        .into_iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
    }

    /// Write loaded (possibly edited) trees back to disk.
    pub fn save(&self) -> crate::Result<()> {
        if let (Some(sch), Some(path)) = (&self.schematic, &self.schematic_path) {
            crate::core::sexp_file::save_schematic(sch, path)?;
        }
        if let (Some(pcb), Some(path)) = (&self.pcb, &self.pcb_path) {
            crate::core::sexp_file::save_pcb(pcb, path)?;
        }
        Ok(())
    }

    /// Mutable access to the loaded trees (load first via `schematic()` /
    /// `pcb()`).
    pub fn schematic_mut(&mut self) -> Option<&mut SExp> {
        self.schematic.as_mut()
    }

    pub fn pcb_mut(&mut self) -> Option<&mut SExp> {
        self.pcb.as_mut()
    }

    /// Python `repr()`.
    pub fn repr(&self) -> String {
        let mut parts = vec![format!("Project({}", py_str_repr(&self.name()))];
        if let Some(s) = &self.schematic_path {
            parts.push(format!("schematic={}", py_str_repr(&file_name(s))));
        }
        if let Some(p) = &self.pcb_path {
            parts.push(format!("pcb={}", py_str_repr(&file_name(p))));
        }
        parts.join(", ") + ")"
    }
}

const PCB_TEMPLATE: &str = r#"(kicad_pcb (version {VERSION}) (generator "kicad_tools") (generator_version "{GENERATOR_VERSION}")

  (general
    (thickness 1.6)
  )

  (paper "A4")

  (layers
    (0 "F.Cu" signal)
    (31 "B.Cu" signal)
    (32 "B.Adhes" user "B.Adhesive")
    (33 "F.Adhes" user "F.Adhesive")
    (34 "B.Paste" user)
    (35 "F.Paste" user)
    (36 "B.SilkS" user "B.Silkscreen")
    (37 "F.SilkS" user "F.Silkscreen")
    (38 "B.Mask" user)
    (39 "F.Mask" user)
    (40 "Dwgs.User" user "User.Drawings")
    (41 "Cmts.User" user "User.Comments")
    (42 "Eco1.User" user "User.Eco1")
    (43 "Eco2.User" user "User.Eco2")
    (44 "Edge.Cuts" user)
    (45 "Margin" user)
    (46 "B.CrtYd" user "B.Courtyard")
    (47 "F.CrtYd" user "F.Courtyard")
    (48 "B.Fab" user)
    (49 "F.Fab" user)
    (50 "User.1" user)
    (51 "User.2" user)
    (52 "User.3" user)
    (53 "User.4" user)
    (54 "User.5" user)
    (55 "User.6" user)
    (56 "User.7" user)
    (57 "User.8" user)
    (58 "User.9" user)
  )

  (setup
    (stackup
      (layer "F.SilkS" (type "Top Silk Screen"))
      (layer "F.Paste" (type "Top Solder Paste"))
      (layer "F.Mask" (type "Top Solder Mask") (thickness 0.01))
      (layer "F.Cu" (type "copper") (thickness 0.035))
      (layer "dielectric 1" (type "core") (thickness 1.51) (material "FR4") (epsilon_r 4.5) (loss_tangent 0.02))
      (layer "B.Cu" (type "copper") (thickness 0.035))
      (layer "B.Mask" (type "Bottom Solder Mask") (thickness 0.01))
      (layer "B.Paste" (type "Bottom Solder Paste"))
      (layer "B.SilkS" (type "Bottom Silk Screen"))
      (copper_finish "None")
    )
    (pad_to_mask_clearance 0)
  )

  (net 0 "")

  (uuid "{PCB_UUID}")

  (gr_rect (start 0 0) (end {WIDTH} {HEIGHT})
    (stroke (width 0.15) (type default))
    (fill none)
    (layer "Edge.Cuts")
    (uuid "{RECT_UUID}")
  )
)
"#;
