//! Hierarchical schematic models (port of `kicad_tools.schema.hierarchy`).
//!
//! Upstream nodes hold a `parent` back-reference; here each node carries its
//! ancestor chain ([`HierarchyNode::ancestors`]) instead, which answers the
//! same questions (`depth`, `is_root`, `get_path_string`, UUID paths).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::sexp::{self, SExp};

use super::symbol::{find_at, find_string, get_float, get_string};

/// A hierarchical label in a (child) schematic.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HierarchicalLabelInfo {
    pub name: String,
    /// input, output, bidirectional, passive, ...
    pub shape: String,
    pub position: (f64, f64),
    pub rotation: f64,
    pub uuid: String,
}

impl HierarchicalLabelInfo {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        HierarchicalLabelInfo {
            name: get_string(sexp, 0).unwrap_or_default(),
            shape: sexp
                .find("shape")
                .and_then(|s| get_string(s, 0))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "input".into()),
            position,
            rotation,
            uuid: find_string(sexp, "uuid"),
        }
    }
}

/// A hierarchical pin on a sheet symbol.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SheetPin {
    pub name: String,
    /// input, output, bidirectional, passive, ...
    pub direction: String,
    pub position: (f64, f64),
    pub rotation: f64,
    pub uuid: String,
}

impl SheetPin {
    pub fn new(
        name: impl Into<String>,
        direction: impl Into<String>,
        position: (f64, f64),
        rotation: f64,
        uuid: impl Into<String>,
    ) -> Self {
        SheetPin {
            name: name.into(),
            direction: direction.into(),
            position,
            rotation,
            uuid: uuid.into(),
        }
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        SheetPin {
            name: get_string(sexp, 0).unwrap_or_default(),
            direction: get_string(sexp, 1)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "passive".into()),
            position,
            rotation,
            uuid: find_string(sexp, "uuid"),
        }
    }
}

/// A hierarchical sheet instance with its pins.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SheetInstance {
    /// Display name (`Sheetname` property).
    pub name: String,
    /// File name (`Sheetfile` property).
    pub filename: String,
    pub uuid: String,
    pub position: (f64, f64),
    pub size: (f64, f64),
    pub pins: Vec<SheetPin>,
}

impl SheetInstance {
    pub fn input_pins(&self) -> Vec<&SheetPin> {
        self.pins
            .iter()
            .filter(|p| p.direction == "input")
            .collect()
    }

    pub fn output_pins(&self) -> Vec<&SheetPin> {
        self.pins
            .iter()
            .filter(|p| p.direction == "output")
            .collect()
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let position = sexp
            .find("at")
            .map(|at| {
                (
                    get_float(at, 0).unwrap_or(0.0),
                    get_float(at, 1).unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0));
        let size = sexp
            .find("size")
            .map(|sz| {
                (
                    get_float(sz, 0).filter(|&v| v != 0.0).unwrap_or(50.8),
                    get_float(sz, 1).filter(|&v| v != 0.0).unwrap_or(25.4),
                )
            })
            .unwrap_or((50.8, 25.4));
        let mut name = String::new();
        let mut filename = String::new();
        for prop in sexp.find_all("property") {
            let value = get_string(prop, 1).unwrap_or_default();
            match get_string(prop, 0).as_deref() {
                Some("Sheetname") => name = value,
                Some("Sheetfile") => filename = value,
                _ => {}
            }
        }
        SheetInstance {
            name,
            filename,
            uuid: find_string(sexp, "uuid"),
            position,
            size,
            pins: sexp.find_all("pin").map(SheetPin::from_sexp).collect(),
        }
    }
}

impl fmt::Display for SheetInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SheetInstance({:?}, file={:?}, pins={})",
            self.name,
            self.filename,
            self.pins.len()
        )
    }
}

/// Name and schematic UUID of an ancestor node.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Ancestor {
    pub name: String,
    pub uuid: String,
    pub path: String,
}

/// A node in the schematic hierarchy tree.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HierarchyNode {
    pub name: String,
    /// Path to the schematic file.
    pub path: String,
    /// The schematic's own UUID.
    pub uuid: String,
    pub children: Vec<HierarchyNode>,
    /// Sheet instances placed in this schematic.
    pub sheets: Vec<SheetInstance>,
    /// Hierarchical label names.
    pub hierarchical_labels: Vec<String>,
    /// Full hierarchical label details.
    pub hierarchical_label_info: Vec<HierarchicalLabelInfo>,
    /// Root-first chain of ancestors (empty for the root).
    pub ancestors: Vec<Ancestor>,
}

impl HierarchyNode {
    pub fn new(name: impl Into<String>, path: impl Into<String>, uuid: impl Into<String>) -> Self {
        HierarchyNode {
            name: name.into(),
            path: path.into(),
            uuid: uuid.into(),
            children: Vec::new(),
            sheets: Vec::new(),
            hierarchical_labels: Vec::new(),
            hierarchical_label_info: Vec::new(),
            ancestors: Vec::new(),
        }
    }

    /// Set the parent (upstream `parent=` argument).
    pub fn with_parent(mut self, parent: &HierarchyNode) -> Self {
        self.ancestors = parent.ancestors.clone();
        self.ancestors.push(Ancestor {
            name: parent.name.clone(),
            uuid: parent.uuid.clone(),
            path: parent.path.clone(),
        });
        self
    }

    pub fn parent(&self) -> Option<&Ancestor> {
        self.ancestors.last()
    }

    /// Depth in the hierarchy (root = 0).
    pub fn depth(&self) -> usize {
        self.ancestors.len()
    }

    pub fn is_root(&self) -> bool {
        self.ancestors.is_empty()
    }

    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// Hierarchical path, e.g. `/`, `/Power`, `/Power/Regulator`.
    pub fn get_path_string(&self) -> String {
        if self.is_root() {
            return "/".into();
        }
        let mut parts: Vec<&str> = self.ancestors[1..]
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        parts.push(&self.name);
        format!("/{}", parts.join("/"))
    }

    /// UUIDs from the root to this node (for `/root/.../self` instance paths).
    pub fn uuid_path(&self) -> Vec<&str> {
        let mut parts: Vec<&str> = self.ancestors.iter().map(|a| a.uuid.as_str()).collect();
        parts.push(&self.uuid);
        parts
    }

    /// Depth-first search of descendants by name.
    pub fn find_by_name(&self, name: &str) -> Option<&HierarchyNode> {
        for child in &self.children {
            if child.name == name {
                return Some(child);
            }
            if let Some(found) = child.find_by_name(name) {
                return Some(found);
            }
        }
        None
    }

    /// Resolve a hierarchical path such as `/Power/Regulator`.
    pub fn find_by_path(&self, path: &str) -> Option<&HierarchyNode> {
        let mut current = self;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            current = current.children.iter().find(|c| c.name == part)?;
        }
        Some(current)
    }

    /// All nodes in pre-order (including self).
    pub fn all_nodes(&self) -> Vec<&HierarchyNode> {
        let mut result = vec![self];
        for child in &self.children {
            result.extend(child.all_nodes());
        }
        result
    }
}

/// Builds a hierarchy tree from schematic files.
#[derive(Debug, Clone)]
pub struct HierarchyBuilder {
    pub base_path: PathBuf,
    loaded_files: HashMap<String, (String, Vec<String>, Vec<HierarchicalLabelInfo>)>,
}

impl HierarchyBuilder {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        HierarchyBuilder {
            base_path: base_path.into(),
            loaded_files: HashMap::new(),
        }
    }

    /// Build the tree starting at `root_schematic` (named `Root`).
    pub fn build(&mut self, root_schematic: impl AsRef<Path>) -> HierarchyNode {
        let root = root_schematic.as_ref();
        self.base_path = root.parent().map(Path::to_path_buf).unwrap_or_default();
        self.load_schematic(&root.to_string_lossy(), "Root", None)
    }

    fn load_schematic(
        &mut self,
        path: &str,
        name: &str,
        parent: Option<&HierarchyNode>,
    ) -> HierarchyNode {
        let mut full_path = PathBuf::from(path);
        if !full_path.exists() && !full_path.is_absolute() {
            full_path = self.base_path.join(path);
        }
        let path_str = full_path.to_string_lossy().into_owned();

        let attach = |node: HierarchyNode| match parent {
            Some(p) => node.with_parent(p),
            None => node,
        };

        if let Some((uuid, labels, info)) = self.loaded_files.get(&path_str) {
            // Already visited (shared or circular sheet): no recursion.
            let mut node = HierarchyNode::new(name, path_str.clone(), uuid.clone());
            node.hierarchical_labels = labels.clone();
            node.hierarchical_label_info = info.clone();
            return attach(node);
        }

        let parsed = std::fs::read_to_string(&full_path)
            .ok()
            .and_then(|text| sexp::parse(&text).ok());
        let Some(root) = parsed else {
            return attach(HierarchyNode::new(name, path_str, ""));
        };

        let uuid = root
            .find("uuid")
            .and_then(|u| get_string(u, 0))
            .unwrap_or_default();
        let sheets: Vec<SheetInstance> = root
            .find_all("sheet")
            .map(SheetInstance::from_sexp)
            .collect();
        let mut labels = Vec::new();
        let mut info = Vec::new();
        for label in root.find_all("hierarchical_label") {
            if let Some(text) = get_string(label, 0).filter(|t| !t.is_empty()) {
                labels.push(text);
                info.push(HierarchicalLabelInfo::from_sexp(label));
            }
        }

        let mut node = attach(HierarchyNode::new(name, path_str.clone(), uuid.clone()));
        node.sheets = sheets.clone();
        node.hierarchical_labels = labels.clone();
        node.hierarchical_label_info = info.clone();
        self.loaded_files.insert(path_str, (uuid, labels, info));

        for sheet in &sheets {
            let child = self.load_schematic(&sheet.filename, &sheet.name, Some(&node));
            node.children.push(child);
        }
        node
    }
}

/// Build a hierarchy tree from a root schematic.
pub fn build_hierarchy(root_schematic: impl AsRef<Path>) -> HierarchyNode {
    let root = root_schematic.as_ref();
    let mut builder =
        HierarchyBuilder::new(root.parent().map(Path::to_path_buf).unwrap_or_default());
    builder.build(root)
}

/// Format a hierarchy tree as text (upstream `print_hierarchy_tree`).
pub fn print_hierarchy_tree(node: &HierarchyNode, indent: &str) -> String {
    let mut lines = Vec::new();
    if node.is_root() {
        let file = Path::new(&node.path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        lines.push(format!("{indent}\u{1F4C1} {file} (root)"));
    } else {
        lines.push(format!("{indent}\u{1F4C4} {}", node.name));
    }
    for label in &node.hierarchical_labels {
        lines.push(format!("{indent}  \u{26A1} {label}"));
    }
    let child_indent = format!("{indent}  ");
    let n = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        let prefix = if i == n - 1 { "└─ " } else { "├─ " };
        let child_text = print_hierarchy_tree(child, &format!("{child_indent}  "));
        let mut child_lines: Vec<String> = child_text.split('\n').map(str::to_string).collect();
        child_lines[0] = format!("{indent}{prefix}{}", child_lines[0].trim_start());
        lines.extend(child_lines);
    }
    lines.join("\n")
}
