//! Port of `kicad_tools.drc.repair_silkscreen`: widen silkscreen strokes and
//! scale silkscreen text up to manufacturer minimums, on the raw s-expression
//! tree so untouched content round-trips byte-exact.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::schema::pcb::is_footprint_tag;
use crate::sexp::{SExp, Value};

pub const SILKSCREEN_LAYERS: &[&str] = &["F.SilkS", "B.SilkS", "F.Silkscreen", "B.Silkscreen"];
pub const FP_GRAPHIC_TYPES: &[&str] = &["fp_line", "fp_rect", "fp_circle", "fp_arc"];
pub const GR_GRAPHIC_TYPES: &[&str] = &["gr_line", "gr_rect", "gr_circle", "gr_arc"];
pub const FP_TEXT_TYPES: &[&str] = &["fp_text", "property"];
pub const GR_TEXT_TYPES: &[&str] = &["gr_text"];

#[derive(Debug, Clone, PartialEq)]
pub struct SilkscreenFix {
    pub element_type: String,
    pub layer: String,
    pub old_width: f64,
    pub new_width: f64,
    pub footprint_ref: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SilkscreenRepairResult {
    pub min_width_mm: f64,
    pub fixes: Vec<SilkscreenFix>,
}

impl SilkscreenRepairResult {
    pub fn total_fixed(&self) -> usize {
        self.fixes.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextHeightFix {
    pub element_type: String,
    pub layer: String,
    pub old_height: f64,
    pub new_height: f64,
    pub old_width: f64,
    pub new_width: f64,
    pub old_thickness: Option<f64>,
    pub new_thickness: Option<f64>,
    pub footprint_ref: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextHeightRepairResult {
    pub min_height_mm: f64,
    pub fixes: Vec<TextHeightFix>,
}

impl TextHeightRepairResult {
    pub fn total_fixed(&self) -> usize {
        self.fixes.len()
    }
}

/// Child-index path from a root to a node.
pub type NodePath = Vec<usize>;

/// Pre-order paths of all descendants (not `root`) matching `pred`.
pub fn descendant_paths(root: &SExp, pred: &dyn Fn(&SExp) -> bool) -> Vec<NodePath> {
    fn walk(node: &SExp, path: &mut NodePath, pred: &dyn Fn(&SExp) -> bool, out: &mut Vec<NodePath>) {
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            if pred(child) {
                out.push(path.clone());
            }
            walk(child, path, pred, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(root, &mut Vec::new(), pred, &mut out);
    out
}

pub fn node_at<'a>(root: &'a SExp, path: &[usize]) -> &'a SExp {
    path.iter().fold(root, |n, &i| &n.children[i])
}

pub fn node_at_mut<'a>(root: &'a mut SExp, path: &[usize]) -> &'a mut SExp {
    path.iter().fold(root, |n, &i| &mut n.children[i])
}

/// Mutable first descendant named `tag`.
pub fn find_mut<'a>(node: &'a mut SExp, tag: &str) -> Option<&'a mut SExp> {
    let path = descendant_paths(node, &|n| n.has_tag(tag)).into_iter().next()?;
    Some(node_at_mut(node, &path))
}

/// `_find_all_footprints`: footprint nodes below the root's children.
pub fn footprint_paths(doc: &SExp) -> Vec<NodePath> {
    descendant_paths(doc, &|n| is_footprint_tag(n.tag()))
}

fn atom_str(v: &Value) -> String {
    v.to_string()
}

/// Silkscreen repairer over a parsed board.
pub struct SilkscreenRepairer {
    pub path: PathBuf,
    pub doc: SExp,
}

impl SilkscreenRepairer {
    pub fn new(pcb_path: &Path) -> Result<Self> {
        Ok(SilkscreenRepairer {
            path: pcb_path.to_path_buf(),
            doc: crate::sexp::parse_file(pcb_path)?,
        })
    }

    pub fn from_doc(doc: SExp, path: &Path) -> Self {
        SilkscreenRepairer {
            path: path.to_path_buf(),
            doc,
        }
    }

    /// Reference designator from `(fp_text reference ...)` or KiCad 8+
    /// `(property "Reference" ...)`.
    pub fn footprint_reference(fp: &SExp) -> String {
        for tag in ["fp_text", "property"] {
            let key = if tag == "fp_text" { "reference" } else { "Reference" };
            for child in &fp.children {
                if child.is_list() && child.has_tag(tag) && !child.children.is_empty() {
                    let atoms: Vec<String> = child.atoms().map(atom_str).collect();
                    if atoms.first().map(String::as_str) == Some(key) && atoms.len() >= 2 {
                        return atoms[1].clone();
                    }
                }
            }
        }
        String::new()
    }

    fn layer_of(node: &SExp) -> Option<String> {
        node.find("layer")?.first_atom().map(atom_str)
    }

    fn is_silkscreen(node: &SExp) -> bool {
        Self::layer_of(node).is_some_and(|l| SILKSCREEN_LAYERS.contains(&l.as_str()))
    }

    fn stroke_width(node: &SExp) -> Option<f64> {
        node.find("stroke")?.find("width")?.first_atom()?.as_f64()
    }

    fn graphic_targets(&self) -> Vec<(NodePath, String, String)> {
        let mut out = Vec::new();
        for fp_path in footprint_paths(&self.doc) {
            let fp = node_at(&self.doc, &fp_path);
            let fp_ref = Self::footprint_reference(fp);
            for gtype in FP_GRAPHIC_TYPES {
                for p in descendant_paths(fp, &|n| n.has_tag(gtype)) {
                    let mut full = fp_path.clone();
                    full.extend(p);
                    out.push((full, gtype.to_string(), fp_ref.clone()));
                }
            }
        }
        for gtype in GR_GRAPHIC_TYPES {
            for p in descendant_paths(&self.doc, &|n| n.has_tag(gtype)) {
                out.push((p, gtype.to_string(), String::new()));
            }
        }
        out
    }

    /// Widen silkscreen strokes below `min_width_mm` (zero widths skipped).
    pub fn repair_line_widths(&mut self, min_width_mm: f64, dry_run: bool) -> SilkscreenRepairResult {
        let mut result = SilkscreenRepairResult {
            min_width_mm,
            fixes: Vec::new(),
        };
        for (path, etype, fp_ref) in self.graphic_targets() {
            let node = node_at(&self.doc, &path);
            if !Self::is_silkscreen(node) {
                continue;
            }
            let Some(width) = Self::stroke_width(node) else {
                continue;
            };
            if width == 0.0 || width >= min_width_mm {
                continue;
            }
            result.fixes.push(SilkscreenFix {
                element_type: etype,
                layer: Self::layer_of(node).unwrap_or_default(),
                old_width: width,
                new_width: min_width_mm,
                footprint_ref: fp_ref,
            });
            if !dry_run {
                let node = node_at_mut(&mut self.doc, &path);
                if let Some(w) = find_mut(node, "stroke").and_then(|s| find_mut(s, "width")) {
                    w.set_value(0, min_width_mm);
                }
            }
        }
        result
    }

    fn is_hidden(node: &SExp) -> bool {
        if let Some(hide) = node.find("hide") {
            match hide.first_atom() {
                Some(a) if atom_str(a) == "yes" => return true,
                None => return true,
                _ => {}
            }
        }
        if node
            .children
            .iter()
            .any(|c| c.is_atom() && c.value.as_ref().is_some_and(|v| atom_str(v) == "hide"))
        {
            return true;
        }
        if let Some(effects) = node.find("effects") {
            if effects
                .children
                .iter()
                .any(|c| c.is_atom() && c.value.as_ref().is_some_and(|v| atom_str(v) == "hide"))
            {
                return true;
            }
            if effects.find("hide").is_some() {
                return true;
            }
        }
        false
    }

    fn font_size(node: &SExp) -> Option<(f64, f64)> {
        let size = node.find("effects")?.find("font")?.find("size")?;
        let atoms: Vec<&Value> = size.atoms().collect();
        if atoms.len() < 2 {
            return None;
        }
        Some((atoms[0].as_f64()?, atoms[1].as_f64()?))
    }

    fn font_thickness(node: &SExp) -> Option<f64> {
        node.find("effects")?
            .find("font")?
            .find("thickness")?
            .first_atom()?
            .as_f64()
    }

    fn text_targets(&self) -> Vec<(NodePath, String, String)> {
        let mut out = Vec::new();
        for fp_path in footprint_paths(&self.doc) {
            let fp = node_at(&self.doc, &fp_path);
            let fp_ref = Self::footprint_reference(fp);
            for ttype in FP_TEXT_TYPES {
                for p in descendant_paths(fp, &|n| n.has_tag(ttype)) {
                    let mut full = fp_path.clone();
                    full.extend(p);
                    out.push((full, ttype.to_string(), fp_ref.clone()));
                }
            }
        }
        for ttype in GR_TEXT_TYPES {
            for p in descendant_paths(&self.doc, &|n| n.has_tag(ttype)) {
                out.push((p, ttype.to_string(), String::new()));
            }
        }
        out
    }

    /// Scale visible silkscreen text below `min_height_mm` (aspect kept).
    pub fn repair_text_heights(&mut self, min_height_mm: f64, dry_run: bool) -> TextHeightRepairResult {
        let mut result = TextHeightRepairResult {
            min_height_mm,
            fixes: Vec::new(),
        };
        for (path, etype, fp_ref) in self.text_targets() {
            let node = node_at(&self.doc, &path);
            if !Self::is_silkscreen(node) || Self::is_hidden(node) {
                continue;
            }
            let Some((fw, fh)) = Self::font_size(node) else {
                continue;
            };
            if fh == 0.0 || fh >= min_height_mm {
                continue;
            }
            let scale = min_height_mm / fh;
            let new_width = crate::pyjson::py_round(fw * scale, 6);
            let new_height = crate::pyjson::py_round(min_height_mm, 6);
            let old_thickness = Self::font_thickness(node);
            let new_thickness = old_thickness
                .filter(|t| *t > 0.0)
                .map(|t| crate::pyjson::py_round(t * scale, 6));
            result.fixes.push(TextHeightFix {
                element_type: etype,
                layer: Self::layer_of(node).unwrap_or_default(),
                old_height: fh,
                new_height,
                old_width: fw,
                new_width,
                old_thickness,
                new_thickness,
                footprint_ref: fp_ref,
            });
            if !dry_run {
                let node = node_at_mut(&mut self.doc, &path);
                if let Some(font) = find_mut(node, "effects").and_then(|e| find_mut(e, "font")) {
                    if let Some(size) = find_mut(font, "size") {
                        size.set_value(0, new_width);
                        size.set_value(1, new_height);
                    }
                    if let Some(t) = new_thickness {
                        if let Some(th) = find_mut(font, "thickness") {
                            th.set_value(0, t);
                        }
                    }
                }
            }
        }
        result
    }

    /// Write the tree (to `output_path` or the source path).
    pub fn save(&self, output_path: Option<&Path>) -> Result<()> {
        crate::core::sexp_file::save_pcb(&self.doc, output_path.unwrap_or(&self.path))
    }
}
