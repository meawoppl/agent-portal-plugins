//! Port of `kicad_tools.schema.pcb`: the KiCad PCB document model.
//!
//! [`Pcb`] parses a `.kicad_pcb` tree into typed element lists (layers,
//! nets, footprints/pads, segments, copper arcs, vias, zones, graphics,
//! setup/stackup, legacy net classes) while keeping the lossless
//! [`Document`] as the source of truth. Every mutation edits the tree, so
//! [`Pcb::save`] writes untouched content exactly as `Document::save` does.
//!
//! # Coordinates
//!
//! All consumer-visible coordinates are in mm and **board-relative**: the
//! minimum corner of the `Edge.Cuts` bounds ([`Pcb::board_origin`]) is
//! subtracted from footprint positions, segment/arc endpoints, via
//! positions, and zone polygons at load time. The tree stays in KiCad's
//! sheet-absolute space; writers add the origin back. Pad positions are
//! footprint-local; pad rotations are absolute board-frame angles (KiCad
//! folds the footprint rotation in), see [`Pad::rotation`].
//!
//! # Format variants
//!
//! Legacy `(module ...)` blocks parse as footprints; KiCad 7 `fp_text` and
//! KiCad 8+ `property` reference/value forms are both read and edited;
//! KiCad 10 name-only `(net "GND")` references are resolved through the
//! header table (synthesized when `--save-board` stripped it), and new
//! copper follows the board's dialect.

mod edit;
mod models;
mod outline;
mod strip;
pub mod util;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde::Serialize;

use crate::core::board_outline::board_outline_bounds;
use crate::sexp::{Document, SExp};
use crate::Result;

pub use edit::{
    CreateOptions, DedupeStats, FootprintMut, NetAssignStats, PadMut, TraceEnd, TraceOptions,
    ViaOptions,
};
pub use models::*;
pub use strip::{StripOptions, StripStats};
pub use util::{canonicalize_power_net, canonicalize_power_nets, fnmatch, is_power_net, new_uuid};

use util::{gf, gi, gs, gs_or, has_atom};

/// Tags that introduce a footprint block (`module` is the pre-KiCad-6 name).
pub const FOOTPRINT_TAGS: &[&str] = &["footprint", "module"];

/// Whether `tag` names a footprint block (modern or legacy).
pub fn is_footprint_tag(tag: Option<&str>) -> bool {
    tag.is_some_and(|t| FOOTPRINT_TAGS.contains(&t))
}

/// Coordinate quantum (decimal places) for copper dedup keys (~1 micron).
pub(crate) const DEDUP_QUANT: i32 = 3;
/// Tighter quantum for UUID-less remove/match round trips through the
/// board origin (0.1 micron).
pub(crate) const COORD_MATCH_QUANT: i32 = 4;

pub(crate) fn quant(v: f64, places: i32) -> i64 {
    (v * 10f64.powi(places)).round() as i64
}

pub(crate) type SegmentKey = (i64, String, ((i64, i64), (i64, i64)), i64);
pub(crate) type ViaKey = (i64, i64, i64, Vec<String>);

/// Order-insensitive dedup key: net, layer, unordered endpoints, width.
pub(crate) fn segment_dedup_key(
    net: i64,
    layer: &str,
    start: Point,
    end: Point,
    width: f64,
) -> SegmentKey {
    let p1 = (quant(start.0, DEDUP_QUANT), quant(start.1, DEDUP_QUANT));
    let p2 = (quant(end.0, DEDUP_QUANT), quant(end.1, DEDUP_QUANT));
    let endpoints = if p1 <= p2 { (p1, p2) } else { (p2, p1) };
    (net, layer.to_string(), endpoints, quant(width, DEDUP_QUANT))
}

/// Via dedup key: net, rounded position, order-insensitive layers.
pub(crate) fn via_dedup_key(net: i64, x: f64, y: f64, layers: &[String]) -> ViaKey {
    let mut l = layers.to_vec();
    l.sort();
    (net, quant(x, DEDUP_QUANT), quant(y, DEDUP_QUANT), l)
}

/// `PCB.summary()` statistics (same keys as upstream).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PcbSummary {
    pub title: String,
    pub revision: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub area_mm2: f64,
    pub copper_layers: usize,
    pub footprints: usize,
    pub nets: usize,
    pub segments: usize,
    pub arcs: usize,
    pub vias: usize,
    pub zones: usize,
    pub trace_length_mm: f64,
}

/// `PCB.routing_status()` result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoutingStatus {
    pub segments: usize,
    pub arcs: usize,
    pub vias: usize,
    pub trace_length_mm: f64,
    pub nets_with_traces: std::collections::BTreeSet<i64>,
    /// `(reference, pad, net_name)` of netted pads on nets with no copper.
    pub unrouted_pads: Vec<(String, String, String)>,
}

/// One `get_ratsnest()` entry: a net with two or more placed pads.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RatsnestEntry {
    pub net: String,
    pub net_number: i64,
    /// `(reference, pad_number, x, y)` board-relative.
    pub pads: Vec<(String, String, f64, f64)>,
}

/// Python `round(v, 2)` for summary fields.
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// KiCad PCB document (upstream `PCB`).
#[derive(Debug, Clone)]
pub struct Pcb {
    pub(crate) doc: Document,
    pub(crate) layers: Vec<Layer>,
    pub(crate) nets: Vec<Net>,
    pub(crate) footprints: Vec<Footprint>,
    pub(crate) segments: Vec<Segment>,
    pub(crate) arcs: Vec<Arc>,
    pub(crate) vias: Vec<Via>,
    pub(crate) segment_keys: Option<HashSet<SegmentKey>>,
    pub(crate) via_keys: Option<HashSet<ViaKey>>,
    pub(crate) zones: Vec<Zone>,
    pub(crate) graphic_lines: Vec<GraphicLine>,
    pub(crate) graphic_arcs: Vec<GraphicArc>,
    pub(crate) texts: Vec<GraphicText>,
    pub(crate) graphics: Vec<BoardGraphic>,
    pub(crate) setup: Option<Setup>,
    pub(crate) net_classes: Vec<BoardNetClass>,
    pub(crate) title_block: Vec<(String, String)>,
    pub(crate) board_origin: Point,
    pub(crate) net_name_only_dialect: bool,
    /// Why the `Edge.Cuts` outline could not prove a board origin (empty
    /// when the bounds resolved or there is no outline). Loading tolerates
    /// malformed outlines; consumers that need one can fail on this.
    pub outline_error: String,
    /// Loud parse diagnostics (e.g. pads found but no readable footprint).
    pub parse_warnings: Vec<String>,
}

impl Pcb {
    // ------------------------------------------------------------ loading

    /// Build from a parsed tree (upstream `PCB(sexp, path)`).
    pub fn new(sexp: SExp, path: Option<PathBuf>) -> Result<Pcb> {
        let mut pcb = Pcb {
            doc: Document { root: sexp, path },
            layers: Vec::new(),
            nets: Vec::new(),
            footprints: Vec::new(),
            segments: Vec::new(),
            arcs: Vec::new(),
            vias: Vec::new(),
            segment_keys: None,
            via_keys: None,
            zones: Vec::new(),
            graphic_lines: Vec::new(),
            graphic_arcs: Vec::new(),
            texts: Vec::new(),
            graphics: Vec::new(),
            setup: None,
            net_classes: Vec::new(),
            title_block: Vec::new(),
            board_origin: (0.0, 0.0),
            net_name_only_dialect: false,
            outline_error: String::new(),
            parse_warnings: Vec::new(),
        };
        pcb.parse()?;
        pcb.detect_board_origin();
        pcb.migrate_legacy_locks();
        Ok(pcb)
    }

    /// Build from a parsed tree with no path.
    pub fn from_sexp(sexp: SExp) -> Result<Pcb> {
        Pcb::new(sexp, None)
    }

    /// Parse board text.
    pub fn parse_str(text: &str) -> Result<Pcb> {
        Pcb::new(crate::parse(text)?, None)
    }

    /// Load a `.kicad_pcb` file (root must be `kicad_pcb`).
    pub fn load(path: impl AsRef<Path>) -> Result<Pcb> {
        let path = path.as_ref();
        if !path.exists() {
            bail!("PCB file not found: {}", path.display());
        }
        let doc = Document::load(path)?;
        if !doc.root.has_tag("kicad_pcb") {
            bail!(
                "Not a KiCad PCB file: {} (got {:?})",
                path.display(),
                doc.root.tag()
            );
        }
        Pcb::new(doc.root, Some(path.to_path_buf()))
    }

    /// Clear derived state and re-derive it from the tree (upstream's
    /// reset + `_parse` + `_detect_board_origin` + link sequence).
    pub(crate) fn reparse(&mut self) -> Result<()> {
        self.layers.clear();
        self.nets.clear();
        self.footprints.clear();
        self.segments.clear();
        self.arcs.clear();
        self.vias.clear();
        self.invalidate_dedup_keys();
        self.zones.clear();
        self.graphic_lines.clear();
        self.graphic_arcs.clear();
        self.texts.clear();
        self.graphics.clear();
        self.setup = None;
        self.net_classes.clear();
        self.title_block.clear();
        self.board_origin = (0.0, 0.0);
        self.parse()?;
        self.detect_board_origin();
        self.migrate_legacy_locks();
        Ok(())
    }

    fn parse(&mut self) -> Result<()> {
        let version = self.file_version();
        let root = std::mem::take(&mut self.doc.root);
        let result = self.parse_children(&root, version);
        self.doc.root = root;
        result?;
        self.fixup_net_numbers();
        self.warn_on_unparsed_component_graph();
        Ok(())
    }

    fn parse_children(&mut self, root: &SExp, version: i64) -> Result<()> {
        for child in root.children.iter().filter(|c| c.is_list()) {
            let Some(tag) = child.tag() else { continue };
            match tag {
                "layers" => self.parse_layers(child),
                "net" => {
                    let number = gi(child, 0).unwrap_or(0);
                    let name = gs(child, 1).unwrap_or_default();
                    self.upsert_net(Net { number, name });
                }
                "footprint" | "module" => self.footprints.push(Footprint::from_sexp(child)),
                "segment" => self.segments.push(Segment::from_sexp(child)),
                "arc" => self
                    .arcs
                    .push(Arc::from_sexp(child).map_err(|e| anyhow!(e))?),
                "via" => self.vias.push(Via::from_sexp(child)),
                "zone" => {
                    let mut zone = Zone::from_sexp(child);
                    zone.file_version = version;
                    self.zones.push(zone);
                }
                "gr_line" => {
                    self.graphic_lines.push(GraphicLine::from_sexp(child));
                    self.graphics.push(BoardGraphic::from_sexp(child, "line"));
                }
                "gr_arc" => {
                    self.graphic_arcs.push(GraphicArc::from_sexp(child));
                    self.graphics.push(BoardGraphic::from_sexp(child, "arc"));
                }
                "setup" => self.setup = Some(parse_setup(child)),
                "title_block" => {
                    for c in child.children.iter().filter(|c| c.is_list()) {
                        let key = c.tag().unwrap_or("").to_string();
                        let value = gs(c, 0).unwrap_or_default();
                        match self.title_block.iter_mut().find(|(k, _)| *k == key) {
                            Some(e) => e.1 = value,
                            None => self.title_block.push((key, value)),
                        }
                    }
                }
                "gr_text" => self.texts.push(GraphicText::from_sexp(child)),
                "gr_rect" | "gr_circle" | "gr_poly" => {
                    self.graphics
                        .push(BoardGraphic::from_sexp(child, &tag[3..]));
                }
                "net_class" => {
                    let nc = parse_net_class(child);
                    match self.net_classes.iter_mut().find(|c| c.name == nc.name) {
                        Some(e) => *e = nc,
                        None => self.net_classes.push(nc),
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_layers(&mut self, sexp: &SExp) {
        for child in sexp.children.iter().filter(|c| c.is_list()) {
            if child.children.is_empty() {
                continue;
            }
            let Some(number) = child.tag().and_then(|t| t.parse::<i64>().ok()) else {
                continue;
            };
            let layer = Layer {
                number,
                name: gs(child, 0).unwrap_or_default(),
                layer_type: gs_or(child, 1, "user"),
            };
            match self.layers.iter_mut().find(|l| l.number == number) {
                Some(e) => *e = layer,
                None => self.layers.push(layer),
            }
        }
    }

    pub(crate) fn upsert_net(&mut self, net: Net) {
        match self.nets.iter_mut().find(|n| n.number == net.number) {
            Some(e) => *e = net,
            None => self.nets.push(net),
        }
    }

    /// The board's `(version N)`, 0 when unreadable.
    pub fn file_version(&self) -> i64 {
        self.doc
            .root
            .find_child("version")
            .and_then(|v| gs(v, 0))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    fn warn_on_unparsed_component_graph(&mut self) {
        if !self.footprints.is_empty() {
            return;
        }
        let pads = self.doc.root.find_all("pad").count();
        if pads == 0 {
            return;
        }
        let mut tags: Vec<String> = self
            .doc
            .root
            .children
            .iter()
            .filter(|c| c.is_list())
            .filter_map(|c| {
                let t = c.tag()?;
                (!is_footprint_tag(Some(t)) && c.find("pad").is_some()).then(|| t.to_string())
            })
            .collect();
        tags.sort();
        tags.dedup();
        let version = self
            .doc
            .root
            .find_child("version")
            .and_then(|v| gs(v, 0))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let listed = if tags.is_empty() {
            "none at top level".to_string()
        } else {
            tags.join(", ")
        };
        self.parse_warnings.push(format!(
            "Board declares version {version} and contains {pads} pad node(s), but no footprint \
             could be parsed: the component graph is unreadable. Unrecognised container tag(s): \
             {listed}. Pad-level results (routing, DRC, LVS, BOM) from this board would be wrong, \
             not merely incomplete."
        ));
    }

    /// Reconcile inline net refs with the header table (KiCad 10 name-only
    /// refs, number-only refs, and `--save-board` boards with no table).
    fn fixup_net_numbers(&mut self) {
        self.net_name_only_dialect = self.segments.iter().any(|s| s.net_name_only.0)
            || self.arcs.iter().any(|a| a.net_name_only.0)
            || self.vias.iter().any(|v| v.net_name_only.0)
            || self
                .footprints
                .iter()
                .flat_map(|f| &f.pads)
                .any(|p| p.net_number == 0 && !p.net_name.is_empty());

        if self.nets.is_empty() {
            self.synthesize_net_table();
        }
        let name_to_number: HashMap<String, i64> = self
            .nets
            .iter()
            .filter(|n| !n.name.is_empty() && n.number != 0)
            .map(|n| (n.name.clone(), n.number))
            .collect();
        if name_to_number.is_empty() && self.nets.is_empty() {
            return;
        }
        let number_to_name: HashMap<i64, String> = self
            .nets
            .iter()
            .map(|n| (n.number, n.name.clone()))
            .collect();
        let fix = |number: &mut i64, name: &mut String| {
            if *number == 0 && !name.is_empty() {
                *number = name_to_number.get(name.as_str()).copied().unwrap_or(0);
            } else if *number != 0 && name.is_empty() {
                if let Some(n) = number_to_name.get(number) {
                    *name = n.clone();
                }
            }
        };
        for fp in &mut self.footprints {
            for pad in &mut fp.pads {
                fix(&mut pad.net_number, &mut pad.net_name);
            }
        }
        for s in &mut self.segments {
            fix(&mut s.net_number, &mut s.net_name);
        }
        for a in &mut self.arcs {
            fix(&mut a.net_number, &mut a.net_name);
        }
        for v in &mut self.vias {
            fix(&mut v.net_number, &mut v.net_name);
        }
        for z in &mut self.zones {
            fix(&mut z.net_number, &mut z.net_name);
        }
    }

    /// Rebuild the net table from inline name-only refs (first-seen order
    /// pads -> segments -> arcs -> vias -> zones, honoring surviving numeric
    /// refs) and write it back into the tree.
    fn synthesize_net_table(&mut self) {
        if !self.nets.is_empty() {
            return;
        }
        let mut name_to_number: HashMap<String, i64> = HashMap::new();
        let mut reserved: HashSet<i64> = HashSet::from([0]);
        let mut ordered: Vec<String> = Vec::new();
        let mut observe = |name: &str, number: i64| {
            if name.is_empty() {
                return;
            }
            if !name_to_number.contains_key(name) {
                ordered.push(name.to_string());
                name_to_number.insert(name.to_string(), 0);
            }
            if number != 0 && name_to_number[name] == 0 {
                name_to_number.insert(name.to_string(), number);
                reserved.insert(number);
            }
        };
        for fp in &self.footprints {
            for p in &fp.pads {
                observe(&p.net_name, p.net_number);
            }
        }
        for s in &self.segments {
            observe(&s.net_name, s.net_number);
        }
        for a in &self.arcs {
            observe(&a.net_name, a.net_number);
        }
        for v in &self.vias {
            observe(&v.net_name, v.net_number);
        }
        for z in &self.zones {
            observe(&z.net_name, z.net_number);
        }
        if ordered.is_empty() {
            return;
        }
        let mut next = 1;
        for name in &ordered {
            if name_to_number[name] != 0 {
                continue;
            }
            while reserved.contains(&next) {
                next += 1;
            }
            name_to_number.insert(name.clone(), next);
            reserved.insert(next);
        }
        self.nets.push(Net {
            number: 0,
            name: String::new(),
        });
        for name in &ordered {
            let number = name_to_number[name];
            self.upsert_net(Net {
                number,
                name: name.clone(),
            });
        }
        self.write_net_declarations();
    }

    /// Insert `(net N "name")` declarations (sorted by number) after the
    /// last top-level `layers`/`setup` node (or at the end).
    fn write_net_declarations(&mut self) {
        let mut sorted: Vec<&Net> = self.nets.iter().collect();
        sorted.sort_by_key(|n| n.number);
        let nodes: Vec<SExp> = sorted
            .iter()
            .map(|n| SExp::list("net", [SExp::atom(n.number), SExp::atom(n.name.as_str())]))
            .collect();
        if nodes.is_empty() {
            return;
        }
        let root = &mut self.doc.root;
        let at = root
            .children
            .iter()
            .rposition(|c| c.has_tag("layers") || c.has_tag("setup"))
            .map(|i| i + 1)
            .unwrap_or(root.children.len());
        for (offset, node) in nodes.into_iter().enumerate() {
            root.insert(at + offset, node);
        }
    }

    /// Detect the board origin from `Edge.Cuts` bounds and convert copper,
    /// zones and footprints to board-relative coordinates. A malformed
    /// outline is tolerated: origin stays (0, 0) and `outline_error` says why.
    fn detect_board_origin(&mut self) {
        let bounds = match board_outline_bounds(&self.doc.root) {
            Ok(b) => b,
            Err(e) => {
                self.outline_error = e.to_string();
                self.board_origin = (0.0, 0.0);
                return;
            }
        };
        self.outline_error.clear();
        let origin = bounds.map(|b| (b.0, b.1)).unwrap_or((0.0, 0.0));
        self.board_origin = origin;
        if origin == (0.0, 0.0) {
            return;
        }
        let (ox, oy) = origin;
        let shift = |p: &mut Point| {
            p.0 -= ox;
            p.1 -= oy;
        };
        for fp in &mut self.footprints {
            shift(&mut fp.position);
        }
        for s in &mut self.segments {
            shift(&mut s.start);
            shift(&mut s.end);
        }
        for a in &mut self.arcs {
            shift(&mut a.start);
            shift(&mut a.mid);
            shift(&mut a.end);
        }
        for v in &mut self.vias {
            shift(&mut v.position);
        }
        for z in &mut self.zones {
            z.polygon.iter_mut().for_each(shift);
            for poly in &mut z.filled_polygons {
                poly.iter_mut().for_each(shift);
            }
        }
    }

    /// Migrate legacy KiCad 6 `(attr smd locked)` to top-level `(locked
    /// yes)` (KiCad 10 rejects the in-attr token) on load.
    fn migrate_legacy_locks(&mut self) {
        let indices = self.footprint_node_indices();
        for (i, node_idx) in indices.into_iter().enumerate() {
            let legacy = self.doc.root.children[node_idx]
                .find_children("attr")
                .first()
                .is_some_and(|attr| has_atom(attr, "locked"));
            if legacy {
                if let Some(mut fm) = self.footprint_mut_at(i) {
                    fm.sync_attr_node();
                }
            }
        }
    }

    // ------------------------------------------------------------ raw tree

    /// The authoritative S-expression tree (sheet-absolute coordinates).
    pub fn sexp(&self) -> &SExp {
        &self.doc.root
    }

    /// Mutable tree access. Typed views are not refreshed; call
    /// [`Pcb::reload_from_tree`] after structural edits.
    pub fn sexp_mut(&mut self) -> &mut SExp {
        &mut self.doc.root
    }

    /// Re-derive every typed view from the current tree.
    pub fn reload_from_tree(&mut self) -> Result<()> {
        self.reparse()
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn into_document(self) -> Document {
        self.doc
    }

    /// Path loaded from / last saved to.
    pub fn path(&self) -> Option<&Path> {
        self.doc.path.as_deref()
    }

    /// Save the tree (untouched text preserved). A given `path` becomes the
    /// stored path. Like upstream `PCB.save` -> `save_pcb`, this checks the
    /// KiCad lock and writes no trailing newline.
    pub fn save(&mut self, path: Option<&Path>) -> Result<()> {
        if !self.doc.root.has_tag("kicad_pcb") {
            bail!("Not a KiCad PCB (root is {:?})", self.doc.root.tag());
        }
        match path {
            Some(p) => self.doc.path = Some(p.to_path_buf()),
            None if self.doc.path.is_none() => bail!(
                "No path specified and PCB has no stored path. Provide a path or use Pcb::load()."
            ),
            None => {}
        }
        let target = self.doc.path.clone().expect("path set above");
        crate::core::sexp_file::save_pcb(&self.doc.root, &target).context("saving PCB")
    }

    /// Indices (into the root's children) of top-level footprint nodes, in
    /// the same order as [`Pcb::footprints`].
    pub(crate) fn footprint_node_indices(&self) -> Vec<usize> {
        self.doc
            .root
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_list() && is_footprint_tag(c.tag()))
            .map(|(i, _)| i)
            .collect()
    }

    pub(crate) fn footprint_node_index(&self, fp_index: usize) -> Option<usize> {
        self.footprint_node_indices().get(fp_index).copied()
    }

    // ------------------------------------------------------------ accessors

    pub fn title(&self) -> &str {
        self.title_field("title")
    }

    pub fn revision(&self) -> &str {
        self.title_field("rev")
    }

    pub fn date(&self) -> &str {
        self.title_field("date")
    }

    /// Any title-block field (`title`, `rev`, `date`, `company`, ...).
    pub fn title_field(&self, key: &str) -> &str {
        self.title_block
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    /// Board origin (min corner of the `Edge.Cuts` bounds) on the sheet.
    pub fn board_origin(&self) -> Point {
        self.board_origin
    }

    /// Whether the board uses KiCad 10 name-only inline net references.
    pub fn net_name_only_dialect(&self) -> bool {
        self.net_name_only_dialect
    }

    /// `(width, height)` of the `Edge.Cuts` bounds; `(0, 0)` without an
    /// outline; an error for malformed outline geometry.
    pub fn board_size(&self) -> Result<Point> {
        match board_outline_bounds(&self.doc.root).map_err(|e| anyhow!(e))? {
            Some(b) => Ok((b.2 - b.0, b.3 - b.1)),
            None => Ok((0.0, 0.0)),
        }
    }

    /// Layer table in file order.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn layer(&self, number: i64) -> Option<&Layer> {
        self.layers.iter().find(|l| l.number == number)
    }

    pub fn layer_by_name(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name == name)
    }

    /// `signal`/`power` layers in file (stack) order.
    pub fn copper_layers(&self) -> Vec<&Layer> {
        self.layers.iter().filter(|l| l.is_copper()).collect()
    }

    /// Net table in declaration order.
    pub fn nets(&self) -> &[Net] {
        &self.nets
    }

    pub fn get_net(&self, number: i64) -> Option<&Net> {
        self.nets.iter().find(|n| n.number == number)
    }

    pub fn get_net_by_name(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }

    pub fn footprints(&self) -> &[Footprint] {
        &self.footprints
    }

    pub fn get_footprint(&self, reference: &str) -> Option<&Footprint> {
        self.footprints.iter().find(|f| f.reference == reference)
    }

    pub fn footprints_on_layer<'a>(
        &'a self,
        layer: &'a str,
    ) -> impl Iterator<Item = &'a Footprint> {
        self.footprints.iter().filter(move |f| f.layer == layer)
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn segments_on_layer<'a>(&'a self, layer: &'a str) -> impl Iterator<Item = &'a Segment> {
        self.segments.iter().filter(move |s| s.layer == layer)
    }

    pub fn segments_in_net(&self, net_number: i64) -> impl Iterator<Item = &Segment> {
        self.segments
            .iter()
            .filter(move |s| s.net_number == net_number)
    }

    /// Imported copper arcs (not included in `segments`).
    pub fn arcs(&self) -> &[Arc] {
        &self.arcs
    }

    pub fn arcs_on_layer<'a>(&'a self, layer: &'a str) -> impl Iterator<Item = &'a Arc> {
        self.arcs.iter().filter(move |a| a.layer == layer)
    }

    pub fn arcs_in_net(&self, net_number: i64) -> impl Iterator<Item = &Arc> {
        self.arcs.iter().filter(move |a| a.net_number == net_number)
    }

    pub fn vias(&self) -> &[Via] {
        &self.vias
    }

    pub fn vias_in_net(&self, net_number: i64) -> impl Iterator<Item = &Via> {
        self.vias.iter().filter(move |v| v.net_number == net_number)
    }

    /// All zones, including rule areas.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    /// Keepout rule areas (zones with a `(keepout ...)` child).
    pub fn rule_areas(&self) -> Vec<&Zone> {
        self.zones.iter().filter(|z| z.keepout.is_some()).collect()
    }

    pub fn graphic_lines(&self) -> &[GraphicLine] {
        &self.graphic_lines
    }

    pub fn graphic_arcs(&self) -> &[GraphicArc] {
        &self.graphic_arcs
    }

    /// Board-level `gr_text`.
    pub fn texts(&self) -> &[GraphicText] {
        &self.texts
    }

    pub fn texts_on_layer<'a>(&'a self, layer: &'a str) -> impl Iterator<Item = &'a GraphicText> {
        self.texts.iter().filter(move |t| t.layer == layer)
    }

    /// Board graphics (`gr_line`/`gr_arc` mirrored here too).
    pub fn graphics(&self) -> &[BoardGraphic] {
        &self.graphics
    }

    pub fn graphics_on_layer<'a>(
        &'a self,
        layer: &'a str,
    ) -> impl Iterator<Item = &'a BoardGraphic> {
        self.graphics.iter().filter(move |g| g.layer == layer)
    }

    /// Lines and arcs in their typed forms, then every other graphic once.
    pub fn graphic_items(&self) -> Vec<GraphicItem<'_>> {
        let mut out: Vec<GraphicItem<'_>> =
            self.graphic_lines.iter().map(GraphicItem::Line).collect();
        out.extend(self.graphic_arcs.iter().map(GraphicItem::Arc));
        out.extend(
            self.graphics
                .iter()
                .filter(|g| g.graphic_type != "line" && g.graphic_type != "arc")
                .map(GraphicItem::Other),
        );
        out
    }

    pub fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()
    }

    /// Legacy board-declared net classes in file order (empty for KiCad 6+).
    pub fn net_classes(&self) -> &[BoardNetClass] {
        &self.net_classes
    }

    pub fn net_class(&self, name: &str) -> Option<&BoardNetClass> {
        self.net_classes.iter().find(|c| c.name == name)
    }

    // ------------------------------------------------------------ statistics

    pub fn footprint_count(&self) -> usize {
        self.footprints.len()
    }

    fn count_top(&self, tag: &str) -> usize {
        self.doc
            .root
            .children
            .iter()
            .filter(|c| c.is_list() && c.has_tag(tag))
            .count()
    }

    /// Top-level `(segment ...)` nodes in the tree.
    pub fn segment_count(&self) -> usize {
        self.count_top("segment")
    }

    pub fn arc_count(&self) -> usize {
        self.count_top("arc")
    }

    pub fn via_count(&self) -> usize {
        self.count_top("via")
    }

    pub fn zone_count(&self) -> usize {
        self.count_top("zone")
    }

    pub fn net_count(&self) -> usize {
        self.nets.len()
    }

    /// Total straight + arc track length in mm, optionally on one layer.
    pub fn total_trace_length(&self, layer: Option<&str>) -> f64 {
        let on = |l: &str| layer.is_none_or(|x| x == l);
        let straight: f64 = self
            .segments
            .iter()
            .filter(|s| on(&s.layer))
            .map(Segment::length)
            .sum();
        let arcs: f64 = self
            .arcs
            .iter()
            .filter(|a| on(&a.layer))
            .map(Arc::length)
            .sum();
        straight + arcs
    }

    /// Board summary statistics (counts from the tree).
    pub fn summary(&self) -> Result<PcbSummary> {
        let (w, h) = self.board_size()?;
        Ok(PcbSummary {
            title: self.title().into(),
            revision: self.revision().into(),
            width_mm: round2(w),
            height_mm: round2(h),
            area_mm2: round2(w * h),
            copper_layers: self.copper_layers().len(),
            footprints: self.footprint_count(),
            nets: self.net_count(),
            segments: self.segment_count(),
            arcs: self.arc_count(),
            vias: self.via_count(),
            zones: self.zone_count(),
            trace_length_mm: round2(self.total_trace_length(None)),
        })
    }

    /// Board-relative position of pad `pad_number` on `reference`
    /// (footprint rotation applied with KiCad's negated-angle convention).
    pub fn get_pad_position(&self, reference: &str, pad_number: &str) -> Option<Point> {
        let fp = self.get_footprint(reference)?;
        let pad = fp.pads.iter().find(|p| p.number == pad_number)?;
        let (rx, ry) =
            crate::core::geometry::rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
        Some((fp.position.0 + rx, fp.position.1 + ry))
    }

    /// Whether any segment of a pad's net ends within 0.01 mm of the pad.
    pub fn footprint_has_traces(&self, reference: &str) -> bool {
        let Some(fp) = self.get_footprint(reference) else {
            return false;
        };
        for pad in &fp.pads {
            if pad.net_number == 0 {
                continue;
            }
            let Some(pos) = self.get_pad_position(reference, &pad.number) else {
                continue;
            };
            for seg in self.segments_in_net(pad.net_number) {
                let ds = (seg.start.0 - pos.0).hypot(seg.start.1 - pos.1);
                let de = (seg.end.0 - pos.0).hypot(seg.end.1 - pos.1);
                if ds < 0.01 || de < 0.01 {
                    return true;
                }
            }
        }
        false
    }

    /// Routing statistics: copper counts, length, nets with copper, and
    /// netted pads whose net has no copper at all.
    pub fn routing_status(&self) -> RoutingStatus {
        let mut nets = std::collections::BTreeSet::new();
        let mut length = 0.0;
        for s in &self.segments {
            length += s.length();
            if s.net_number > 0 {
                nets.insert(s.net_number);
            }
        }
        for a in &self.arcs {
            length += a.length();
            if a.net_number > 0 {
                nets.insert(a.net_number);
            }
        }
        for v in &self.vias {
            if v.net_number > 0 {
                nets.insert(v.net_number);
            }
        }
        let mut unrouted = Vec::new();
        for fp in &self.footprints {
            for pad in &fp.pads {
                if pad.net_number > 0 && !nets.contains(&pad.net_number) {
                    unrouted.push((
                        fp.reference.clone(),
                        pad.number.clone(),
                        pad.net_name.clone(),
                    ));
                }
            }
        }
        RoutingStatus {
            segments: self.segment_count(),
            arcs: self.arc_count(),
            vias: self.via_count(),
            trace_length_mm: length,
            nets_with_traces: nets,
            unrouted_pads: unrouted,
        }
    }

    /// Nets with two or more placed pads, in first-seen order.
    pub fn get_ratsnest(&self) -> Vec<RatsnestEntry> {
        let mut order: Vec<i64> = Vec::new();
        let mut pads: HashMap<i64, Vec<(String, String, f64, f64)>> = HashMap::new();
        for fp in &self.footprints {
            for pad in &fp.pads {
                if pad.net_number <= 0 {
                    continue;
                }
                if let Some(pos) = self.get_pad_position(&fp.reference, &pad.number) {
                    let entry = pads.entry(pad.net_number).or_insert_with(|| {
                        order.push(pad.net_number);
                        Vec::new()
                    });
                    entry.push((fp.reference.clone(), pad.number.clone(), pos.0, pos.1));
                }
            }
        }
        order
            .into_iter()
            .filter_map(|n| {
                let p = pads.remove(&n)?;
                (p.len() >= 2).then(|| RatsnestEntry {
                    net: self.get_net(n).map(|x| x.name.clone()).unwrap_or_default(),
                    net_number: n,
                    pads: p,
                })
            })
            .collect()
    }

    // --------------------------------------------------------- dedup keys

    pub(crate) fn invalidate_dedup_keys(&mut self) {
        self.segment_keys = None;
        self.via_keys = None;
    }

    pub(crate) fn ensure_segment_keys(&mut self) -> &mut HashSet<SegmentKey> {
        if self.segment_keys.is_none() {
            self.segment_keys = Some(
                self.segments
                    .iter()
                    .map(|s| segment_dedup_key(s.net_number, &s.layer, s.start, s.end, s.width))
                    .collect(),
            );
        }
        self.segment_keys.as_mut().unwrap()
    }

    pub(crate) fn ensure_via_keys(&mut self) -> &mut HashSet<ViaKey> {
        if self.via_keys.is_none() {
            self.via_keys = Some(
                self.vias
                    .iter()
                    .map(|v| via_dedup_key(v.net_number, v.position.0, v.position.1, &v.layers))
                    .collect(),
            );
        }
        self.via_keys.as_mut().unwrap()
    }
}

fn parse_setup(sexp: &SExp) -> Setup {
    let mut setup = Setup::default();
    if let Some(stackup) = sexp.find("stackup") {
        setup.stackup = parse_stackup(stackup);
    }
    if let Some(c) = sexp.find("pad_to_mask_clearance") {
        setup.pad_to_mask_clearance = gf(c, 0).unwrap_or(0.0);
    }
    if let Some(a) = sexp.find("aux_axis_origin") {
        setup.aux_axis_origin = (gf(a, 0).unwrap_or(0.0), gf(a, 1).unwrap_or(0.0));
    }
    if let Some(t) = sexp.find("tenting") {
        let side = |tag: &str| match t.find(tag).and_then(|n| gs(n, 0)).as_deref() {
            Some("yes") => Some(true),
            Some("no") => Some(false),
            _ => None,
        };
        setup.tenting_front = side("front");
        setup.tenting_back = side("back");
    }
    setup
}

/// Physical strata, expanding `addsublayer` composite dielectrics.
fn parse_stackup(sexp: &SExp) -> Vec<StackupLayer> {
    let mut layers = Vec::new();
    for child in sexp.children.iter().filter(|c| c.has_tag("layer")) {
        let name = gs(child, 0).unwrap_or_default();
        let layer_type = child
            .find("type")
            .and_then(|t| gs(t, 0))
            .unwrap_or_default();
        let mut layer = StackupLayer {
            name: name.clone(),
            layer_type: layer_type.clone(),
            ..Default::default()
        };
        let mut sub = 1;
        for item in &child.children {
            if item.is_atom() && item.value.as_ref().and_then(|v| v.as_str()) == Some("addsublayer")
            {
                layers.push(std::mem::take(&mut layer));
                sub += 1;
                layer = StackupLayer {
                    name: format!("{name} (sublayer {sub})"),
                    layer_type: layer_type.clone(),
                    ..Default::default()
                };
            } else if item.has_tag("thickness") {
                layer.thickness = gf(item, 0).unwrap_or(0.0);
            } else if item.has_tag("material") {
                layer.material = gs(item, 0).unwrap_or_default();
            } else if item.has_tag("epsilon_r") {
                layer.epsilon_r = gf(item, 0).unwrap_or(0.0);
            } else if item.has_tag("loss_tangent") {
                layer.loss_tangent = gf(item, 0).unwrap_or(0.0);
            }
        }
        layers.push(layer);
    }
    layers
}

fn parse_net_class(sexp: &SExp) -> BoardNetClass {
    let mut nc = BoardNetClass {
        name: gs(sexp, 0).unwrap_or_default(),
        description: gs(sexp, 1).unwrap_or_default(),
        ..Default::default()
    };
    let dim = |tag: &str| sexp.find(tag).and_then(|n| gf(n, 0));
    nc.clearance = dim("clearance");
    nc.trace_width = dim("trace_width");
    nc.via_dia = dim("via_dia");
    nc.via_drill = dim("via_drill");
    nc.uvia_dia = dim("uvia_dia");
    nc.uvia_drill = dim("uvia_drill");
    nc.diff_pair_width = dim("diff_pair_width");
    nc.diff_pair_gap = dim("diff_pair_gap");
    nc.nets = sexp.find_all("add_net").filter_map(|n| gs(n, 0)).collect();
    nc
}
