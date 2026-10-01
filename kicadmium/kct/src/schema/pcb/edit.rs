//! Mutating half of the PCB model: write-through footprint/pad handles,
//! reference/value/silkscreen edits, footprint placement from library
//! files, nets, traces, vias, removals, and board creation.

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::Path;

use anyhow::{anyhow, bail};

use crate::core::version::{KICAD_BOARD_FORMAT_VERSION, KICAD_GENERATOR_VERSION};
use crate::sexp::{SExp, Value};
use crate::Result;

use super::models::*;
use super::util::{
    at_path_mut, atom, descendant_paths, fnmatch, for_each_named_mut, gf, gs, list, new_uuid,
    pair_xy, string_atoms, Untracked,
};
use super::{is_footprint_tag, quant, segment_dedup_key, via_dedup_key, Pcb, COORD_MATCH_QUANT};

// ================================================================ handles

/// Write-through handle to one footprint: setters update both the typed
/// value and its `(footprint ...)` node (Python `Footprint.__setattr__`).
pub struct FootprintMut<'a> {
    fp: &'a mut Footprint,
    node: Option<&'a mut SExp>,
    origin: Point,
}

impl Deref for FootprintMut<'_> {
    type Target = Footprint;
    fn deref(&self) -> &Footprint {
        self.fp
    }
}

impl FootprintMut<'_> {
    /// Board-relative position; the tree gets `pos + board_origin`.
    pub fn set_position(&mut self, pos: Point) {
        self.fp.position = pos;
        if let Some(at) = self.node.as_deref_mut().and_then(|n| n.get_mut("at")) {
            at.set_value(0, pos.0 + self.origin.0);
            at.set_value(1, pos.1 + self.origin.1);
        }
    }

    pub fn set_rotation(&mut self, rotation: f64) {
        self.fp.rotation = rotation;
        if let Some(at) = self.node.as_deref_mut().and_then(|n| n.get_mut("at")) {
            super::models::sync_at_angle(&mut self.fp.at_angle_synthetic.0, at, rotation);
        }
    }

    pub fn set_layer(&mut self, layer: &str) {
        self.fp.layer = layer.to_string();
        if let Some(l) = self.node.as_deref_mut().and_then(|n| n.get_mut("layer")) {
            l.set_value(0, layer);
        }
    }

    /// `smd`, `through_hole` or empty.
    pub fn set_attr(&mut self, attr: &str) {
        self.fp.attr = attr.to_string();
        self.sync_attr_node();
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.fp.locked = locked;
        self.sync_attr_node();
    }

    pub fn set_dnp(&mut self, dnp: bool) {
        self.fp.dnp = dnp;
        self.sync_attr_node();
    }

    pub fn set_exclude_from_bom(&mut self, v: bool) {
        self.fp.exclude_from_bom = v;
        self.sync_attr_node();
    }

    pub fn set_exclude_from_pos_files(&mut self, v: bool) {
        self.fp.exclude_from_pos_files = v;
        self.sync_attr_node();
    }

    pub fn set_attr_unknown_tokens(&mut self, tokens: Vec<String>) {
        self.fp.attr_unknown_tokens = Untracked(tokens);
        self.sync_attr_node();
    }

    /// Rebuild `(attr ...)` from the flags and re-emit the lock as a
    /// top-level `(locked yes)` (direct children only; pad locks untouched).
    pub(crate) fn sync_attr_node(&mut self) {
        let Some(node) = self.node.as_deref_mut() else {
            return;
        };
        node.retain(|c| !(c.has_tag("attr") || c.has_tag("locked")));
        if let Some(attr) = self.fp.attr_node() {
            node.push(attr);
        }
        if self.fp.locked {
            node.push(list("locked", vec![atom("yes")]));
        }
    }

    pub fn pad_count(&self) -> usize {
        self.fp.pads.len()
    }

    /// Write-through handle to the `index`-th pad (parser order).
    pub fn pad_mut(&mut self, index: usize) -> Option<PadMut<'_>> {
        let node = match self.node.as_deref_mut() {
            Some(n) => {
                let path = descendant_paths(n, "pad").into_iter().nth(index);
                path.and_then(move |p| at_path_mut(n, &p))
            }
            None => None,
        };
        let pad = self.fp.pads.get_mut(index)?;
        Some(PadMut { pad, node })
    }
}

/// Write-through handle to one pad (rotation, local position, layers).
pub struct PadMut<'a> {
    pad: &'a mut Pad,
    node: Option<&'a mut SExp>,
}

impl Deref for PadMut<'_> {
    type Target = Pad;
    fn deref(&self) -> &Pad {
        self.pad
    }
}

impl PadMut<'_> {
    /// Absolute board-frame angle.
    pub fn set_rotation(&mut self, rotation: f64) {
        self.pad.rotation = rotation;
        if let Some(at) = self.node.as_deref_mut().and_then(|n| n.get_mut("at")) {
            super::models::sync_at_angle(&mut self.pad.at_angle_synthetic.0, at, rotation);
        }
    }

    /// Footprint-local position.
    pub fn set_position(&mut self, pos: Point) {
        self.pad.position = pos;
        if let Some(at) = self.node.as_deref_mut().and_then(|n| n.get_mut("at")) {
            at.set_value(0, pos.0);
            at.set_value(1, pos.1);
        }
    }

    pub fn set_layers(&mut self, layers: &[&str]) {
        self.pad.layers = layers.iter().map(|s| s.to_string()).collect();
        if let Some(node) = self.node.as_deref_mut().and_then(|n| n.get_mut("layers")) {
            node.children = layers.iter().map(|l| SExp::quoted(*l)).collect();
        }
    }
}

/// Trace endpoint: a board-relative point or a `(reference, pad)` pair.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceEnd {
    Point(Point),
    Pad(String, String),
}

impl From<Point> for TraceEnd {
    fn from(p: Point) -> Self {
        TraceEnd::Point(p)
    }
}

impl From<(&str, &str)> for TraceEnd {
    fn from((r, p): (&str, &str)) -> Self {
        TraceEnd::Pad(r.into(), p.into())
    }
}

/// `add_trace` keyword arguments (upstream defaults).
#[derive(Debug, Clone)]
pub struct TraceOptions {
    pub width: f64,
    pub layer: String,
    /// Net name; auto-detected from pad endpoints when `None`.
    pub net: Option<String>,
    pub waypoints: Vec<Point>,
    /// Skip exact-duplicate copper (net, layer, unordered ends, width).
    pub dedupe: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            width: 0.25,
            layer: "F.Cu".into(),
            net: None,
            waypoints: Vec::new(),
            dedupe: true,
        }
    }
}

impl TraceOptions {
    pub fn net(net: &str) -> Self {
        TraceOptions {
            net: Some(net.into()),
            ..Default::default()
        }
    }
}

/// `add_via` keyword arguments (upstream defaults).
#[derive(Debug, Clone)]
pub struct ViaOptions {
    pub size: f64,
    pub drill: f64,
    pub layers: Vec<String>,
    pub net: Option<String>,
    pub dedupe: bool,
}

impl Default for ViaOptions {
    fn default() -> Self {
        ViaOptions {
            size: 0.6,
            drill: 0.3,
            layers: vec!["F.Cu".into(), "B.Cu".into()],
            net: None,
            dedupe: true,
        }
    }
}

impl ViaOptions {
    pub fn net(net: &str) -> Self {
        ViaOptions {
            net: Some(net.into()),
            ..Default::default()
        }
    }
}

/// `PCB.create()` arguments (upstream defaults).
#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub width: f64,
    pub height: f64,
    /// 2 or 4 copper layers.
    pub layers: u32,
    pub title: String,
    pub revision: String,
    pub company: String,
    /// `YYYY-MM-DD`; today when `None`.
    pub board_date: Option<String>,
    pub paper: String,
    /// Plain-center the outline on the paper (no title-block inset).
    pub center: bool,
}

impl Default for CreateOptions {
    fn default() -> Self {
        CreateOptions {
            width: 100.0,
            height: 100.0,
            layers: 2,
            title: String::new(),
            revision: "1.0".into(),
            company: String::new(),
            board_date: None,
            paper: "A4".into(),
            center: true,
        }
    }
}

impl CreateOptions {
    pub fn size(width: f64, height: f64) -> Self {
        CreateOptions {
            width,
            height,
            ..Default::default()
        }
    }
}

/// `assign_nets_from_netlist` statistics.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct NetAssignStats {
    /// `REF.PAD` assigned.
    pub assigned: Vec<String>,
    pub missing_footprints: Vec<String>,
    /// `REF.PIN` not found.
    pub missing_pads: Vec<String>,
}

/// `dedupe_copper` result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DedupeStats {
    pub segments: usize,
    pub vias: usize,
}

/// Today's date as `YYYY-MM-DD` (UTC).
fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Reference designator of a footprint node (KiCad 7 `fp_text`, then 8+
/// `property`), `""` when absent.
pub(crate) fn footprint_reference(fp: &SExp) -> String {
    if let Some(t) = fp
        .find_all("fp_text")
        .find(|t| gs(t, 0).as_deref() == Some("reference"))
    {
        return gs(t, 1).unwrap_or_default();
    }
    if let Some(p) = fp
        .find_all("property")
        .find(|p| gs(p, 0).as_deref() == Some("Reference"))
    {
        return gs(p, 1).unwrap_or_default();
    }
    String::new()
}

fn default_effects() -> SExp {
    list(
        "effects",
        vec![list(
            "font",
            vec![
                list("size", vec![atom(1.0), atom(1.0)]),
                list("thickness", vec![atom(0.15)]),
            ],
        )],
    )
}

fn set_text_visibility(text: &mut SExp, visible: bool) {
    let path = descendant_paths(text, "effects").into_iter().next();
    let effects = match path {
        Some(p) => at_path_mut(text, &p).unwrap(),
        None => {
            text.push(default_effects());
            text.children.last_mut().unwrap()
        }
    };
    let has_hide = effects.find("hide").is_some();
    if visible {
        if has_hide {
            effects.remove_child("hide");
        }
    } else if !has_hide {
        effects.push(list("hide", vec![atom("yes")]));
    }
}

fn move_text_element(text: &mut SExp, offset: Point, absolute: Option<Point>, layer: Option<&str>) {
    let at = match descendant_paths(text, "at").into_iter().next() {
        Some(p) => at_path_mut(text, &p).unwrap(),
        None => {
            text.push(pair_xy("at", 0.0, 0.0));
            text.children.last_mut().unwrap()
        }
    };
    match absolute {
        Some((x, y)) => {
            at.set_value(0, x);
            at.set_value(1, y);
        }
        None => {
            let (x, y) = (gf(at, 0).unwrap_or(0.0), gf(at, 1).unwrap_or(0.0));
            at.set_value(0, x + offset.0);
            at.set_value(1, y + offset.1);
        }
    }
    if let Some(layer) = layer {
        set_or_append_layer(text, layer);
    }
}

fn set_or_append_layer(text: &mut SExp, layer: &str) {
    match descendant_paths(text, "layer").into_iter().next() {
        Some(p) => at_path_mut(text, &p).unwrap().set_value(0, layer),
        None => text.children.push(list("layer", vec![atom(layer)])),
    }
}

fn first_named_mut<'a>(node: &'a mut SExp, tag: &str) -> Option<&'a mut SExp> {
    let path = descendant_paths(node, tag).into_iter().next()?;
    at_path_mut(node, &path)
}

fn set_text_font(text: &mut SExp, size: Point, thickness: f64) {
    if first_named_mut(text, "effects").is_none() {
        text.push(list("effects", vec![]));
    }
    let effects = first_named_mut(text, "effects").unwrap();
    if first_named_mut(effects, "font").is_none() {
        effects.push(list("font", vec![]));
    }
    let font = first_named_mut(effects, "font").unwrap();
    match first_named_mut(font, "size") {
        Some(s) => {
            s.set_value(0, size.0);
            s.set_value(1, size.1);
        }
        None => font
            .children
            .push(list("size", vec![atom(size.0), atom(size.1)])),
    }
    match first_named_mut(font, "thickness") {
        Some(t) => t.set_value(0, thickness),
        None => font.children.push(list("thickness", vec![atom(thickness)])),
    }
}

impl Pcb {
    // ============================================================ creation

    /// New blank board: layers, setup, net 0, and a four-`gr_line`
    /// `Edge.Cuts` rectangle (plain-centered on the paper by default).
    pub fn create(opts: CreateOptions) -> Result<Pcb> {
        if opts.layers != 2 && opts.layers != 4 {
            bail!("Layers must be 2 or 4, got {}", opts.layers);
        }
        let Some((pw, ph)) = paper_size(&opts.paper) else {
            let mut names: Vec<&str> = PAPER_SIZES.iter().map(|(n, _)| *n).collect();
            names.sort();
            bail!(
                "Unknown paper size '{}'. Supported sizes: {}",
                opts.paper,
                names.join(", ")
            );
        };
        let date = opts.board_date.clone().unwrap_or_else(today_iso);
        let (ox, oy) = if opts.center {
            ((pw - opts.width) / 2.0, (ph - opts.height) / 2.0)
        } else {
            (0.0, 0.0)
        };
        let mut root = list("kicad_pcb", vec![]);
        root.push(list("version", vec![atom(KICAD_BOARD_FORMAT_VERSION)]));
        root.push(list("generator", vec![atom("kicad_tools")]));
        root.push(list(
            "generator_version",
            vec![SExp::quoted(KICAD_GENERATOR_VERSION)],
        ));
        root.push(list(
            "general",
            vec![
                list("thickness", vec![atom(1.6)]),
                list("legacy_teardrops", vec![atom("no")]),
            ],
        ));
        root.push(list("paper", vec![atom(opts.paper.as_str())]));
        root.push(list(
            "title_block",
            vec![
                list("title", vec![atom(opts.title.as_str())]),
                list("date", vec![atom(date.as_str())]),
                list("rev", vec![atom(opts.revision.as_str())]),
                list("company", vec![atom(opts.company.as_str())]),
            ],
        ));
        root.push(build_layers_sexp(opts.layers));
        root.push(build_setup_sexp(opts.layers));
        root.push(list("net", vec![atom(0i64), atom("")]));
        for line in build_board_outline_sexp(opts.width, opts.height, ox, oy) {
            root.push(line);
        }
        Pcb::new(root, None)
    }

    // ============================================================ handles

    /// Write-through handle to the `index`-th footprint.
    pub fn footprint_mut_at(&mut self, index: usize) -> Option<FootprintMut<'_>> {
        let node_idx = self.footprint_node_index(index);
        let origin = self.board_origin;
        let node = node_idx.and_then(|i| self.doc.root.children.get_mut(i));
        let fp = self.footprints.get_mut(index)?;
        Some(FootprintMut { fp, node, origin })
    }

    /// Write-through handle to the first footprint with `reference`.
    pub fn footprint_mut(&mut self, reference: &str) -> Option<FootprintMut<'_>> {
        let index = self
            .footprints
            .iter()
            .position(|f| f.reference == reference)?;
        self.footprint_mut_at(index)
    }

    /// Move a footprint (board-relative) and optionally rotate it.
    pub fn update_footprint_position(
        &mut self,
        reference: &str,
        x: f64,
        y: f64,
        rotation: Option<f64>,
    ) -> bool {
        let Some(mut fm) = self.footprint_mut(reference) else {
            return false;
        };
        fm.set_position((x, y));
        if let Some(r) = rotation {
            fm.set_rotation(r);
        }
        true
    }

    fn footprint_nodes_mut(&mut self) -> impl Iterator<Item = &mut SExp> {
        self.doc
            .root
            .children
            .iter_mut()
            .filter(|c| c.is_list() && is_footprint_tag(c.tag()))
    }

    /// Rename a reference designator (fp_text and property forms). Fails
    /// when `old` is missing or `new` already exists.
    pub fn update_footprint_reference(&mut self, old: &str, new: &str) -> bool {
        if self.get_footprint(old).is_none() {
            return false;
        }
        if old != new && self.get_footprint(new).is_some() {
            return false;
        }
        let mut done = false;
        for node in self.footprint_nodes_mut() {
            if footprint_reference(node) != old {
                continue;
            }
            for_each_named_mut(node, "fp_text", &mut |t| {
                if gs(t, 0).as_deref() == Some("reference") {
                    t.set_value(1, new);
                }
            });
            for_each_named_mut(node, "property", &mut |p| {
                if gs(p, 0).as_deref() == Some("Reference") {
                    p.set_value(1, new);
                }
            });
            done = true;
            break;
        }
        if done {
            if let Some(fp) = self.footprints.iter_mut().find(|f| f.reference == old) {
                fp.reference = new.to_string();
                for t in fp.texts.iter_mut().filter(|t| t.text_type == "reference") {
                    t.text = new.to_string();
                }
            }
        }
        done
    }

    /// Change a footprint's value field (fp_text and property forms).
    pub fn update_footprint_value(&mut self, reference: &str, value: &str) -> bool {
        if self.get_footprint(reference).is_none() {
            return false;
        }
        let mut done = false;
        for node in self.footprint_nodes_mut() {
            if footprint_reference(node) != reference {
                continue;
            }
            for_each_named_mut(node, "fp_text", &mut |t| {
                if gs(t, 0).as_deref() == Some("value") {
                    t.set_value(1, value);
                }
            });
            for_each_named_mut(node, "property", &mut |p| {
                if gs(p, 0).as_deref() == Some("Value") {
                    p.set_value(1, value);
                }
            });
            done = true;
            break;
        }
        if done {
            if let Some(fp) = self
                .footprints
                .iter_mut()
                .find(|f| f.reference == reference)
            {
                fp.value = value.to_string();
                for t in fp.texts.iter_mut().filter(|t| t.text_type == "value") {
                    t.text = value.to_string();
                }
            }
        }
        done
    }

    fn select_refs(&self, reference: Option<&str>, pattern: Option<&str>) -> HashSet<String> {
        match (reference, pattern) {
            (Some(r), _) => HashSet::from([r.to_string()]),
            (None, Some(p)) => self
                .footprints
                .iter()
                .filter(|f| fnmatch(&f.reference, p))
                .map(|f| f.reference.clone())
                .collect(),
            (None, None) => self
                .footprints
                .iter()
                .map(|f| f.reference.clone())
                .collect(),
        }
    }

    /// Show/hide reference designators: one `reference`, a glob `pattern`,
    /// or all. Returns the number of text nodes updated.
    pub fn set_reference_visibility(
        &mut self,
        reference: Option<&str>,
        visible: bool,
        pattern: Option<&str>,
    ) -> usize {
        let refs = self.select_refs(reference, pattern);
        let mut count = 0;
        for node in self.footprint_nodes_mut() {
            if !refs.contains(&footprint_reference(node)) {
                continue;
            }
            for_each_named_mut(node, "fp_text", &mut |t| {
                if gs(t, 0).as_deref() == Some("reference") {
                    set_text_visibility(t, visible);
                    count += 1;
                }
            });
            for_each_named_mut(node, "property", &mut |p| {
                if gs(p, 0).as_deref() == Some("Reference") {
                    set_text_visibility(p, visible);
                    count += 1;
                }
            });
        }
        for fp in self
            .footprints
            .iter_mut()
            .filter(|f| refs.contains(&f.reference))
        {
            for t in fp.texts.iter_mut().filter(|t| t.text_type == "reference") {
                t.hidden = !visible;
            }
        }
        count
    }

    /// Move a reference text by `offset`, or to footprint-local `absolute`,
    /// optionally changing its layer.
    pub fn move_reference(
        &mut self,
        reference: &str,
        offset: Point,
        absolute: Option<Point>,
        layer: Option<&str>,
    ) -> bool {
        let mut updated = false;
        let mut found = false;
        for node in self.footprint_nodes_mut() {
            if footprint_reference(node) != reference {
                continue;
            }
            found = true;
            for_each_named_mut(node, "fp_text", &mut |t| {
                if gs(t, 0).as_deref() == Some("reference") {
                    move_text_element(t, offset, absolute, layer);
                    updated = true;
                }
            });
            for_each_named_mut(node, "property", &mut |p| {
                if gs(p, 0).as_deref() == Some("Reference") {
                    move_text_element(p, offset, absolute, layer);
                    updated = true;
                }
            });
            break;
        }
        if !found {
            return false;
        }
        if updated {
            if let Some(fp) = self
                .footprints
                .iter_mut()
                .find(|f| f.reference == reference)
            {
                for t in fp.texts.iter_mut().filter(|t| t.text_type == "reference") {
                    t.position = match absolute {
                        Some(p) => p,
                        None => (t.position.0 + offset.0, t.position.1 + offset.1),
                    };
                    if let Some(l) = layer {
                        t.layer = l.to_string();
                    }
                }
            }
        }
        updated
    }

    /// Set font size/thickness of `text_types` (`reference`, `value`,
    /// `user`) on all or glob-matched footprints.
    pub fn set_silkscreen_font(
        &mut self,
        size: Point,
        thickness: f64,
        pattern: Option<&str>,
        text_types: &[&str],
    ) -> usize {
        let refs = self.select_refs(None, pattern);
        let mut count = 0;
        for node in self.footprint_nodes_mut() {
            if !refs.contains(&footprint_reference(node)) {
                continue;
            }
            for_each_named_mut(node, "fp_text", &mut |t| {
                if gs(t, 0).is_some_and(|k| text_types.contains(&k.as_str())) {
                    set_text_font(t, size, thickness);
                    count += 1;
                }
            });
            for_each_named_mut(node, "property", &mut |p| {
                let name = gs(p, 0);
                if (name.as_deref() == Some("Reference") && text_types.contains(&"reference"))
                    || (name.as_deref() == Some("Value") && text_types.contains(&"value"))
                {
                    set_text_font(p, size, thickness);
                    count += 1;
                }
            });
        }
        for fp in self
            .footprints
            .iter_mut()
            .filter(|f| refs.contains(&f.reference))
        {
            for t in fp
                .texts
                .iter_mut()
                .filter(|t| text_types.contains(&t.text_type.as_str()))
            {
                t.font_size = size;
                t.font_thickness = thickness;
            }
        }
        count
    }

    /// Move reference texts of all or glob-matched footprints to `layer`.
    pub fn move_references_to_layer(&mut self, layer: &str, pattern: Option<&str>) -> usize {
        let refs = self.select_refs(None, pattern);
        let mut count = 0;
        for node in self.footprint_nodes_mut() {
            if !refs.contains(&footprint_reference(node)) {
                continue;
            }
            for_each_named_mut(node, "fp_text", &mut |t| {
                if gs(t, 0).as_deref() == Some("reference") {
                    set_or_append_layer(t, layer);
                    count += 1;
                }
            });
            for_each_named_mut(node, "property", &mut |p| {
                if gs(p, 0).as_deref() == Some("Reference") {
                    set_or_append_layer(p, layer);
                    count += 1;
                }
            });
        }
        for fp in self
            .footprints
            .iter_mut()
            .filter(|f| refs.contains(&f.reference))
        {
            for t in fp.texts.iter_mut().filter(|t| t.text_type == "reference") {
                t.layer = layer.to_string();
            }
        }
        count
    }

    // ============================================================ footprints

    /// Place a `.kicad_mod` library footprint at board-relative `(x, y)`.
    ///
    /// Fresh footprint and pad UUIDs are minted (legacy `tstamp` dropped),
    /// `(at)` is inserted right after `(layer)`, pad angles become absolute
    /// (`(local + rotation) % 360`), and Reference/Value are written quoted
    /// (synthesized as properties when the library has neither form).
    #[allow(clippy::too_many_arguments)]
    pub fn add_footprint_from_file(
        &mut self,
        kicad_mod_path: impl AsRef<Path>,
        reference: &str,
        x: f64,
        y: f64,
        rotation: f64,
        layer: &str,
        value: &str,
    ) -> Result<&Footprint> {
        let path = kicad_mod_path.as_ref();
        if !path.exists() {
            bail!("Footprint file not found: {}", path.display());
        }
        let mut fp = crate::sexp::parse_file(path)?;
        if !(fp.has_tag("footprint") || fp.has_tag("module")) {
            bail!(
                "Not a KiCad footprint file: {} (got {:?})",
                path.display(),
                fp.tag()
            );
        }
        match fp.get_mut("uuid") {
            Some(u) => u.set_value(0, new_uuid()),
            None => fp.children.push(list("uuid", vec![atom(new_uuid())])),
        }
        for_each_named_mut(&mut fp, "pad", &mut |pad| {
            match pad.get_mut("uuid") {
                Some(u) => u.set_value(0, new_uuid()),
                None => pad.children.push(list("uuid", vec![atom(new_uuid())])),
            }
            pad.remove_child("tstamp");
        });
        match first_named_mut(&mut fp, "layer") {
            Some(l) => l.set_value(0, layer),
            None => fp.children.push(list("layer", vec![atom(layer)])),
        }
        // Upstream removes the first `at` descendant if it is (equal to) a
        // direct child: library footprints carry no top-level (at).
        if let Some(first_at) = fp.find("at").cloned() {
            if let Some(i) = fp.children.iter().position(|c| *c == first_at) {
                fp.children.remove(i);
            }
        }
        let (ox, oy) = self.board_origin;
        let mut at = pair_xy("at", x + ox, y + oy);
        if rotation != 0.0 {
            at.push(atom(rotation));
        }
        match fp
            .children
            .iter()
            .position(|c| c.is_list() && c.has_tag("layer"))
        {
            Some(i) => fp.children.insert(i + 1, at),
            None => fp.children.push(at),
        }
        if rotation != 0.0 {
            for_each_named_mut(&mut fp, "pad", &mut |pad| {
                if let Some(pat) = first_named_mut(pad, "at") {
                    let local = gf(pat, 2).unwrap_or(0.0);
                    pat.set_value(2, (local + rotation).rem_euclid(360.0));
                }
            });
        }
        let (mut ref_done, mut val_done) = (false, false);
        for_each_named_mut(&mut fp, "property", &mut |p| {
            let name = gs(p, 0);
            let slot = match name.as_deref() {
                Some("Reference") => {
                    ref_done = true;
                    reference
                }
                Some("Value") => {
                    val_done = true;
                    value
                }
                _ => return,
            };
            if p.children.len() > 1 {
                p.children[1] = SExp::quoted(slot);
            } else {
                p.push(SExp::quoted(slot));
            }
        });
        for_each_named_mut(&mut fp, "fp_text", &mut |t| {
            let kind = gs(t, 0);
            let slot = match kind.as_deref() {
                Some("reference") if !ref_done => {
                    ref_done = true;
                    reference
                }
                Some("value") if !val_done => {
                    val_done = true;
                    value
                }
                _ => return,
            };
            if t.children.len() > 1 {
                t.children[1] = SExp::quoted(slot);
            } else {
                t.push(SExp::quoted(slot));
            }
        });
        let synth = |name: &str, text: &str, y: f64, layer: String| {
            list(
                "property",
                vec![
                    atom(name),
                    SExp::quoted(text),
                    pair_xy("at", 0.0, y),
                    list("layer", vec![atom(layer)]),
                    list("uuid", vec![atom(new_uuid())]),
                    default_effects(),
                ],
            )
        };
        if !ref_done {
            fp.push(synth(
                "Reference",
                reference,
                -1.5,
                layer.replace(".Cu", ".SilkS"),
            ));
        }
        if !val_done {
            fp.push(synth("Value", value, 1.5, layer.replace(".Cu", ".Fab")));
        }
        let mut parsed = Footprint::from_sexp(&fp);
        parsed.position = (x, y);
        self.doc.root.push(fp);
        self.footprints.push(parsed);
        Ok(self.footprints.last().unwrap())
    }

    /// Remove the first footprint with `reference` from tree and model.
    pub fn remove_footprint(&mut self, reference: &str) -> bool {
        let indices = self.footprint_node_indices();
        let Some((k, &node_idx)) = indices
            .iter()
            .enumerate()
            .find(|(_, &i)| footprint_reference(&self.doc.root.children[i]) == reference)
        else {
            return false;
        };
        self.doc.root.children.remove(node_idx);
        if k < self.footprints.len() {
            self.footprints.remove(k);
        }
        true
    }

    // ============================================================ nets

    /// Add a net (or return the existing one with that name). The
    /// declaration goes after the last top-level `(net ...)`.
    pub fn add_net(&mut self, name: &str) -> Net {
        if let Some(n) = self.get_net_by_name(name) {
            return n.clone();
        }
        let number = self.nets.iter().map(|n| n.number).max().unwrap_or(0) + 1;
        let net = Net {
            number,
            name: name.to_string(),
        };
        self.nets.push(net.clone());
        let node = list("net", vec![atom(number), atom(name)]);
        let root = &mut self.doc.root;
        if let Some(i) = root.children.iter().rposition(|c| c.has_tag("net")) {
            root.children.insert(i + 1, node);
        } else if let Some(i) = root.children.iter().position(|c| is_footprint_tag(c.tag())) {
            root.children.insert(i, node);
        } else {
            root.push(node);
        }
        net
    }

    /// Assign `net_name` (created if needed) to every pad numbered
    /// `pad_number` on `reference` (shared-number EP groups included).
    pub fn assign_net_to_footprint_pad(
        &mut self,
        reference: &str,
        pad_number: &str,
        net_name: &str,
    ) -> bool {
        if self.get_footprint(reference).is_none() {
            return false;
        }
        let net = self.add_net(net_name);
        let fp = self
            .footprints
            .iter_mut()
            .find(|f| f.reference == reference)
            .unwrap();
        let mut found = false;
        for pad in fp.pads.iter_mut().filter(|p| p.number == pad_number) {
            pad.net_number = net.number;
            pad.net_name = net.name.clone();
            found = true;
        }
        if !found {
            return false;
        }
        for node in self.footprint_nodes_mut() {
            if footprint_reference(node) != reference {
                continue;
            }
            let mut sexp_found = false;
            for_each_named_mut(node, "pad", &mut |pad| {
                if gs(pad, 0).as_deref() == Some(pad_number) {
                    if let Some(p) = descendant_paths(pad, "net").into_iter().next() {
                        // Upstream `remove` only succeeds on a direct child.
                        if p.len() == 1 {
                            pad.children.remove(p[0]);
                        }
                    }
                    pad.push(list("net", vec![atom(net.number), atom(net.name.as_str())]));
                    sexp_found = true;
                }
            });
            return sexp_found;
        }
        false
    }

    /// Assign nets from netlist connectivity: `nets` is `(net_name,
    /// [(reference, pin)])`; `pin_to_pad` translates schematic pins.
    pub fn assign_nets_from_netlist(
        &mut self,
        nets: &[(String, Vec<(String, String)>)],
        pin_to_pad: Option<&HashMap<(String, String), String>>,
    ) -> NetAssignStats {
        let mut stats = NetAssignStats::default();
        let mut warned = HashSet::new();
        for (name, nodes) in nets {
            if name.is_empty() {
                continue;
            }
            for (reference, pin) in nodes {
                let pad = pin_to_pad
                    .and_then(|m| m.get(&(reference.clone(), pin.clone())))
                    .cloned()
                    .unwrap_or_else(|| pin.clone());
                if self.get_footprint(reference).is_none() {
                    if warned.insert(reference.clone()) {
                        stats.missing_footprints.push(reference.clone());
                    }
                    continue;
                }
                if self.assign_net_to_footprint_pad(reference, &pad, name) {
                    stats.assigned.push(format!("{reference}.{pad}"));
                } else {
                    stats.missing_pads.push(format!("{reference}.{pin}"));
                }
            }
        }
        stats
    }

    // ============================================================ copper

    fn resolve_end(&self, end: &TraceEnd, net: &mut Option<String>) -> Result<Point> {
        match end {
            TraceEnd::Point(p) => Ok(*p),
            TraceEnd::Pad(r, p) => {
                let pos = self
                    .get_pad_position(r, p)
                    .ok_or_else(|| anyhow!("Cannot find pad {p} on footprint {r}"))?;
                if net.is_none() {
                    if let Some(fp) = self.get_footprint(r) {
                        if let Some(pad) = fp
                            .pads
                            .iter()
                            .find(|x| x.number == *p && !x.net_name.is_empty())
                        {
                            *net = Some(pad.net_name.clone());
                        }
                    }
                }
                Ok(pos)
            }
        }
    }

    /// Add a trace `start -> waypoints -> end` (board-relative). Pad
    /// endpoints resolve through [`Pcb::get_pad_position`] and supply the
    /// net when none is given. Exact duplicates are skipped when
    /// `opts.dedupe`, so the result may be shorter than the point pairs.
    pub fn add_trace(
        &mut self,
        start: impl Into<TraceEnd>,
        end: impl Into<TraceEnd>,
        opts: TraceOptions,
    ) -> Result<Vec<Segment>> {
        let mut net = opts.net.clone();
        let start = self.resolve_end(&start.into(), &mut net)?;
        let end = self.resolve_end(&end.into(), &mut net)?;
        let net = net.filter(|n| !n.is_empty());
        let net_number = match &net {
            Some(n) => self.add_net(n).number,
            None => 0,
        };
        let mut points = vec![start];
        points.extend(opts.waypoints.iter().copied());
        points.push(end);
        let origin = self.board_origin;
        let name_only = self.net_name_only_dialect && net.is_some();
        let mut out = Vec::new();
        for w in points.windows(2) {
            if opts.dedupe {
                let key = segment_dedup_key(net_number, &opts.layer, w[0], w[1], opts.width);
                if !self.ensure_segment_keys().insert(key) {
                    continue;
                }
            }
            let mut seg = Segment {
                start: w[0],
                end: w[1],
                width: opts.width,
                layer: opts.layer.clone(),
                net_number,
                net_name: net.clone().unwrap_or_default(),
                uuid: new_uuid(),
                net_name_only: Untracked(name_only),
            };
            let node = seg.to_sexp(origin);
            self.doc.root.push(node);
            self.segments.push(seg.clone());
            out.push(seg);
        }
        Ok(out)
    }

    /// Add a via at board-relative `(x, y)`; `None` when an identical via
    /// (net, rounded position, layer set) exists and `opts.dedupe`.
    pub fn add_via(&mut self, x: f64, y: f64, opts: ViaOptions) -> Option<Via> {
        let net = opts.net.clone().filter(|n| !n.is_empty());
        let net_number = match &net {
            Some(n) => self.add_net(n).number,
            None => 0,
        };
        if opts.dedupe {
            let key = via_dedup_key(net_number, x, y, &opts.layers);
            if !self.ensure_via_keys().insert(key) {
                return None;
            }
        }
        let mut via = Via {
            position: (x, y),
            size: opts.size,
            drill: opts.drill,
            layers: opts.layers.clone(),
            net_number,
            net_name: net.clone().unwrap_or_default(),
            uuid: new_uuid(),
            via_type: None,
            net_name_only: Untracked(self.net_name_only_dialect && net.is_some()),
            tenting_front: None,
            tenting_back: None,
        };
        let node = via.to_sexp(self.board_origin);
        self.doc.root.push(node);
        self.vias.push(via.clone());
        Some(via)
    }

    /// Move `vias()[index]` to a board-relative position. Matched in the
    /// tree by UUID, else by its current position. Returns whether a tree
    /// node was updated (the model is updated regardless).
    pub fn relocate_via(&mut self, index: usize, new_position: Point) -> bool {
        let Some(via) = self.vias.get(index) else {
            return false;
        };
        let (ox, oy) = self.board_origin;
        let (old_x, old_y) = via.position;
        let target = via.uuid.clone();
        let mut updated = false;
        for child in self.doc.root.children.iter_mut() {
            if !(child.is_list() && child.has_tag("via")) {
                continue;
            }
            let Some(at_path) = descendant_paths(child, "at").into_iter().next() else {
                continue;
            };
            if !target.is_empty() {
                let uuid = child
                    .find("uuid")
                    .and_then(|u| gs(u, 0))
                    .unwrap_or_default();
                if uuid != target {
                    continue;
                }
            } else {
                let at = at_path_mut(child, &at_path).unwrap();
                let (ax, ay) = (gf(at, 0).unwrap_or(0.0), gf(at, 1).unwrap_or(0.0));
                if (ax - (old_x + ox)).abs() > 1e-6 || (ay - (old_y + oy)).abs() > 1e-6 {
                    continue;
                }
            }
            let at = at_path_mut(child, &at_path).unwrap();
            at.set_value(0, new_position.0 + ox);
            at.set_value(1, new_position.1 + oy);
            updated = true;
            break;
        }
        self.vias[index].position = new_position;
        self.invalidate_dedup_keys();
        updated
    }

    fn coord_key(&self, p: Point) -> (i64, i64) {
        let (ox, oy) = self.board_origin;
        (
            quant(p.0 + ox, COORD_MATCH_QUANT),
            quant(p.1 + oy, COORD_MATCH_QUANT),
        )
    }

    /// Remove segments (by UUID, else by rounded sheet-absolute endpoints +
    /// layer) from tree and model. Returns tree nodes removed.
    pub fn remove_segments(&mut self, segments: &[Segment]) -> usize {
        if segments.is_empty() {
            return 0;
        }
        let mut uuids = HashSet::new();
        let mut coords = HashSet::new();
        for s in segments {
            if s.uuid.is_empty() {
                coords.insert((
                    self.coord_key(s.start),
                    self.coord_key(s.end),
                    s.layer.clone(),
                ));
            } else {
                uuids.insert(s.uuid.clone());
            }
        }
        let q = |p: Point| (quant(p.0, COORD_MATCH_QUANT), quant(p.1, COORD_MATCH_QUANT));
        let before = self.doc.root.children.len();
        self.doc.root.children.retain(|child| {
            if !(child.is_list() && child.has_tag("segment")) {
                return true;
            }
            if let Some(u) = child.find("uuid") {
                if uuids.contains(&gs(u, 0).unwrap_or_default()) {
                    return false;
                }
            }
            if let (Some(s), Some(e), Some(l)) =
                (child.find("start"), child.find("end"), child.find("layer"))
            {
                let key = (
                    q(super::util::xy(s)),
                    q(super::util::xy(e)),
                    gs(l, 0).unwrap_or_default(),
                );
                if coords.contains(&key) {
                    return false;
                }
            }
            true
        });
        let removed = before - self.doc.root.children.len();
        let origin_key = |s: &Segment| {
            let (ox, oy) = self.board_origin;
            let k = |p: Point| {
                (
                    quant(p.0 + ox, COORD_MATCH_QUANT),
                    quant(p.1 + oy, COORD_MATCH_QUANT),
                )
            };
            (k(s.start), k(s.end), s.layer.clone())
        };
        let keep: Vec<bool> = self
            .segments
            .iter()
            .map(|s| {
                if !s.uuid.is_empty() {
                    !uuids.contains(&s.uuid)
                } else {
                    !coords.contains(&origin_key(s))
                }
            })
            .collect();
        let mut it = keep.into_iter();
        self.segments.retain(|_| it.next().unwrap());
        self.invalidate_dedup_keys();
        removed
    }

    /// Remove vias (by UUID, else rounded sheet-absolute position + layer
    /// list) from tree and model. Returns tree nodes removed.
    pub fn remove_vias(&mut self, vias: &[Via]) -> usize {
        if vias.is_empty() {
            return 0;
        }
        let mut uuids = HashSet::new();
        let mut coords = HashSet::new();
        for v in vias {
            if v.uuid.is_empty() {
                coords.insert((self.coord_key(v.position), v.layers.clone()));
            } else {
                uuids.insert(v.uuid.clone());
            }
        }
        let before = self.doc.root.children.len();
        self.doc.root.children.retain(|child| {
            if !(child.is_list() && child.has_tag("via")) {
                return true;
            }
            if let Some(u) = child.find("uuid") {
                if uuids.contains(&gs(u, 0).unwrap_or_default()) {
                    return false;
                }
            }
            if let (Some(at), Some(l)) = (child.find("at"), child.find("layers")) {
                let key = (
                    (
                        quant(gf(at, 0).unwrap_or(0.0), COORD_MATCH_QUANT),
                        quant(gf(at, 1).unwrap_or(0.0), COORD_MATCH_QUANT),
                    ),
                    string_atoms(l),
                );
                if coords.contains(&key) {
                    return false;
                }
            }
            true
        });
        let removed = before - self.doc.root.children.len();
        let keep: Vec<bool> = self
            .vias
            .iter()
            .map(|v| {
                if !v.uuid.is_empty() {
                    !uuids.contains(&v.uuid)
                } else {
                    !coords.contains(&(self.coord_key(v.position), v.layers.clone()))
                }
            })
            .collect();
        let mut it = keep.into_iter();
        self.vias.retain(|_| it.next().unwrap());
        self.invalidate_dedup_keys();
        removed
    }

    /// Translate every segment endpoint within `tolerance` of `point` by
    /// `delta` (board-relative), in model and tree, keeping UUIDs.
    /// `net_number > 0` restricts to that net. Returns endpoints moved.
    pub fn drag_trace_endpoints(
        &mut self,
        point: Point,
        delta: Point,
        tolerance: f64,
        net_number: Option<i64>,
    ) -> usize {
        let (ox, oy) = self.board_origin;
        let tol_sq = tolerance * tolerance;
        let mut by_uuid: HashMap<String, usize> = HashMap::new();
        let mut by_coord: Vec<(CoordKey, usize)> = Vec::new();
        for (i, child) in self.doc.root.children.iter().enumerate() {
            if !(child.is_list() && child.has_tag("segment")) {
                continue;
            }
            let uuid = child
                .find("uuid")
                .and_then(|u| gs(u, 0))
                .unwrap_or_default();
            if !uuid.is_empty() {
                by_uuid.insert(uuid, i);
                continue;
            }
            if let (Some(s), Some(e), Some(l)) =
                (child.find("start"), child.find("end"), child.find("layer"))
            {
                let (sx, sy) = super::util::xy(s);
                let (ex, ey) = super::util::xy(e);
                by_coord.push(((sx, sy, ex, ey, gs(l, 0).unwrap_or_default()), i));
            }
        }
        let hit = |c: Point| (c.0 - point.0).powi(2) + (c.1 - point.1).powi(2) <= tol_sq;
        let mut moved = 0;
        for k in 0..self.segments.len() {
            let seg = &self.segments[k];
            if let Some(n) = net_number {
                if n > 0 && seg.net_number != n {
                    continue;
                }
            }
            let (hs, he) = (hit(seg.start), hit(seg.end));
            if !hs && !he {
                continue;
            }
            let node_idx = if !seg.uuid.is_empty() {
                by_uuid.get(&seg.uuid).copied()
            } else {
                let key = (
                    seg.start.0 + ox,
                    seg.start.1 + oy,
                    seg.end.0 + ox,
                    seg.end.1 + oy,
                    seg.layer.clone(),
                );
                by_coord.iter().find(|(c, _)| *c == key).map(|(_, i)| *i)
            };
            let seg = &mut self.segments[k];
            for (is_hit, tag) in [(hs, "start"), (he, "end")] {
                if !is_hit {
                    continue;
                }
                let p = if tag == "start" {
                    &mut seg.start
                } else {
                    &mut seg.end
                };
                *p = (p.0 + delta.0, p.1 + delta.1);
                let np = *p;
                if let Some(i) = node_idx {
                    if let Some(n) = first_named_mut(&mut self.doc.root.children[i], tag) {
                        set_atom(n, 0, np.0 + ox);
                        set_atom(n, 1, np.1 + oy);
                    }
                }
                moved += 1;
            }
        }
        if moved > 0 {
            self.invalidate_dedup_keys();
        }
        moved
    }

    /// Drop exact-duplicate copper (first of each dedup key kept) from model
    /// and tree; connectivity is unchanged.
    pub fn dedupe_copper(&mut self) -> DedupeStats {
        let mut seg_uuids = HashSet::new();
        let mut seen = HashSet::new();
        let before_s = self.segments.len();
        self.segments.retain(|s| {
            let key = segment_dedup_key(s.net_number, &s.layer, s.start, s.end, s.width);
            if seen.insert(key) {
                true
            } else {
                if !s.uuid.is_empty() {
                    seg_uuids.insert(s.uuid.clone());
                }
                false
            }
        });
        let mut via_uuids = HashSet::new();
        let mut seen_v = HashSet::new();
        let before_v = self.vias.len();
        self.vias.retain(|v| {
            let key = via_dedup_key(v.net_number, v.position.0, v.position.1, &v.layers);
            if seen_v.insert(key) {
                true
            } else {
                if !v.uuid.is_empty() {
                    via_uuids.insert(v.uuid.clone());
                }
                false
            }
        });
        if !seg_uuids.is_empty() || !via_uuids.is_empty() {
            self.doc.root.children.retain(|c| {
                let set = if c.has_tag("segment") {
                    &seg_uuids
                } else if c.has_tag("via") {
                    &via_uuids
                } else {
                    return true;
                };
                !c.find("uuid")
                    .and_then(|u| gs(u, 0))
                    .is_some_and(|u| set.contains(&u))
            });
        }
        self.invalidate_dedup_keys();
        DedupeStats {
            segments: before_s - self.segments.len(),
            vias: before_v - self.vias.len(),
        }
    }
}

/// Sheet-absolute `(sx, sy, ex, ey, layer)` of a UUID-less segment.
type CoordKey = (f64, f64, f64, f64, String);

/// Python `SExp.set_atom(i, v)`: replace the `i`-th atom child.
fn set_atom(node: &mut SExp, index: usize, value: f64) {
    let mut seen = 0;
    for c in node.children.iter_mut() {
        if c.is_atom() {
            if seen == index {
                *c = SExp::atom(Value::Float(value));
                return;
            }
            seen += 1;
        }
    }
}

pub(crate) fn build_layers_sexp(num_layers: u32) -> SExp {
    let l = |n: &str, name: &str, kind: &str, user: Option<&str>| {
        let mut c = vec![atom(name), atom(kind)];
        if let Some(u) = user {
            c.push(atom(u));
        }
        list(n, c)
    };
    let mut node = list("layers", vec![l("0", "F.Cu", "signal", None)]);
    if num_layers == 4 {
        node.push(l("1", "In1.Cu", "signal", None));
        node.push(l("2", "In2.Cu", "signal", None));
    }
    for (n, name, user) in [
        ("31", "B.Cu", None),
        ("32", "B.Adhes", Some("B.Adhesive")),
        ("33", "F.Adhes", Some("F.Adhesive")),
        ("34", "B.Paste", None),
        ("35", "F.Paste", None),
        ("36", "B.SilkS", Some("B.Silkscreen")),
        ("37", "F.SilkS", Some("F.Silkscreen")),
        ("38", "B.Mask", None),
        ("39", "F.Mask", None),
        ("40", "Dwgs.User", Some("User.Drawings")),
        ("44", "Edge.Cuts", None),
        ("46", "B.CrtYd", Some("B.Courtyard")),
        ("47", "F.CrtYd", Some("F.Courtyard")),
        ("48", "B.Fab", None),
        ("49", "F.Fab", None),
    ] {
        let kind = if name == "B.Cu" { "signal" } else { "user" };
        node.push(l(n, name, kind, user));
    }
    node
}

pub(crate) fn build_setup_sexp(num_layers: u32) -> SExp {
    let mut setup = list("setup", vec![]);
    if num_layers == 4 {
        let layer = |name: &str, kind: &str, extra: Vec<SExp>| {
            let mut c = vec![atom(name), list("type", vec![atom(kind)])];
            c.extend(extra);
            list("layer", c)
        };
        let th = |v: f64| list("thickness", vec![atom(v)]);
        let dielectric = |name: &str, kind: &str, t: f64| {
            layer(
                name,
                kind,
                vec![
                    th(t),
                    list("material", vec![atom("FR4")]),
                    list("epsilon_r", vec![atom(4.5)]),
                    list("loss_tangent", vec![atom(0.02)]),
                ],
            )
        };
        let stackup = list(
            "stackup",
            vec![
                layer("F.SilkS", "Top Silk Screen", vec![]),
                layer("F.Paste", "Top Solder Paste", vec![]),
                layer("F.Mask", "Top Solder Mask", vec![th(0.01)]),
                layer("F.Cu", "copper", vec![th(0.035)]),
                dielectric("dielectric 1", "prepreg", 0.2),
                layer("In1.Cu", "copper", vec![th(0.035)]),
                dielectric("dielectric 2", "core", 1.0),
                layer("In2.Cu", "copper", vec![th(0.035)]),
                dielectric("dielectric 3", "prepreg", 0.2),
                layer("B.Cu", "copper", vec![th(0.035)]),
                layer("B.Mask", "Bottom Solder Mask", vec![th(0.01)]),
                layer("B.Paste", "Bottom Solder Paste", vec![]),
                layer("B.SilkS", "Bottom Silk Screen", vec![]),
                list("copper_finish", vec![atom("ENIG")]),
                list("dielectric_constraints", vec![atom("no")]),
            ],
        );
        setup.push(stackup);
    }
    setup.push(list("pad_to_mask_clearance", vec![atom(0i64)]));
    setup
}

/// Four clockwise `gr_line` `Edge.Cuts` segments of a rectangle.
pub(crate) fn build_board_outline_sexp(width: f64, height: f64, ox: f64, oy: f64) -> Vec<SExp> {
    let corners = [
        (ox, oy),
        (ox + width, oy),
        (ox + width, oy + height),
        (ox, oy + height),
    ];
    (0..4)
        .map(|i| {
            let (s, e) = (corners[i], corners[(i + 1) % 4]);
            list(
                "gr_line",
                vec![
                    pair_xy("start", s.0, s.1),
                    pair_xy("end", e.0, e.1),
                    list(
                        "stroke",
                        vec![
                            list("width", vec![atom(0.1)]),
                            list("type", vec![atom("default")]),
                        ],
                    ),
                    list("layer", vec![atom("Edge.Cuts")]),
                    list("uuid", vec![atom(new_uuid())]),
                ],
            )
        })
        .collect()
}
