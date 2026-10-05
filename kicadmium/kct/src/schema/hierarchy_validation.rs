//! Hierarchy validation (port of `kicad_tools.schema.hierarchy_validation`).
//!
//! Checks sheet pins against child hierarchical labels, reporting missing
//! labels, orphan labels, and direction mismatches with fix suggestions.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use super::hierarchy::{build_hierarchy, HierarchicalLabelInfo, SheetPin};
use super::symbol::OrderedMap;

/// Types of hierarchy validation issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationIssueType {
    /// Sheet pin exists but no label in child.
    MissingLabel,
    /// Label exists but no pin in parent (orphan).
    MissingPin,
    /// Label direction doesn't match pin.
    DirectionMismatch,
    /// Similar names but not exact match.
    NameMismatch,
}

impl ValidationIssueType {
    pub fn value(self) -> &'static str {
        match self {
            ValidationIssueType::MissingLabel => "missing_label",
            ValidationIssueType::MissingPin => "missing_pin",
            ValidationIssueType::DirectionMismatch => "direction_mismatch",
            ValidationIssueType::NameMismatch => "name_mismatch",
        }
    }
}

/// Types of automatic fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FixType {
    AddLabel,
    RemovePin,
    FixDirection,
    RenameLabel,
    RenamePin,
}

impl FixType {
    pub fn value(self) -> &'static str {
        match self {
            FixType::AddLabel => "add_label",
            FixType::RemovePin => "remove_pin",
            FixType::FixDirection => "fix_direction",
            FixType::RenameLabel => "rename_label",
            FixType::RenamePin => "rename_pin",
        }
    }
}

/// A suggested fix for a validation issue.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FixSuggestion {
    pub fix_type: FixType,
    pub description: String,
    pub file_path: String,
    pub auto_fixable: bool,
    pub details: OrderedMap<String>,
}

impl FixSuggestion {
    pub fn new(
        fix_type: FixType,
        description: impl Into<String>,
        file_path: impl Into<String>,
    ) -> Self {
        FixSuggestion {
            fix_type,
            description: description.into(),
            file_path: file_path.into(),
            auto_fixable: false,
            details: OrderedMap::new(),
        }
    }

    fn with(mut self, auto_fixable: bool, details: &[(&str, &str)]) -> Self {
        self.auto_fixable = auto_fixable;
        for (k, v) in details {
            self.details.insert(*k, v.to_string());
        }
        self
    }
}

impl fmt::Display for FixSuggestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.auto_fixable {
            "[Auto-fixable] "
        } else {
            ""
        };
        write!(f, "{prefix}{}", self.description)
    }
}

/// A single hierarchy validation issue.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ValidationIssue {
    pub issue_type: ValidationIssueType,
    pub sheet_name: String,
    pub sheet_file: String,
    pub parent_sheet_name: String,
    pub parent_sheet_file: String,
    pub pin_name: Option<String>,
    pub label_name: Option<String>,
    pub pin: Option<SheetPin>,
    pub label: Option<HierarchicalLabelInfo>,
    pub message: String,
    pub suggestions: Vec<FixSuggestion>,
    pub possible_causes: Vec<String>,
}

impl ValidationIssue {
    /// `error` for missing labels, `warning` otherwise.
    pub fn severity(&self) -> &'static str {
        match self.issue_type {
            ValidationIssueType::MissingLabel => "error",
            ValidationIssueType::MissingPin
            | ValidationIssueType::DirectionMismatch
            | ValidationIssueType::NameMismatch => "warning",
        }
    }
}

/// Result of hierarchy validation.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ValidationResult {
    pub root_schematic: String,
    pub issues: Vec<ValidationIssue>,
    pub sheets_checked: usize,
    pub pins_checked: usize,
    pub labels_checked: usize,
}

impl ValidationResult {
    pub fn new(root_schematic: impl Into<String>) -> Self {
        ValidationResult {
            root_schematic: root_schematic.into(),
            ..Default::default()
        }
    }

    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity() == "error")
    }

    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity() == "error")
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity() == "warning")
            .count()
    }

    pub fn issues_for_sheet(&self, sheet_file: &str) -> Vec<&ValidationIssue> {
        self.issues
            .iter()
            .filter(|i| i.sheet_file == sheet_file)
            .collect()
    }
}

/// Whether a sheet pin direction is compatible with a label shape
/// (case-insensitive; bidirectional/passive match anything).
pub fn directions_compatible(pin_direction: &str, label_shape: &str) -> bool {
    let pin = pin_direction.to_lowercase();
    let label = label_shape.to_lowercase();
    let flexible = |d: &str| d == "bidirectional" || d == "passive";
    pin == label || flexible(&pin) || flexible(&label)
}

// ------------------------------------------------ difflib.SequenceMatcher

/// `difflib.SequenceMatcher(None, a, b).ratio()` over Unicode scalar values.
pub fn sequence_ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matches: usize = matching_blocks(&a, &b).iter().map(|&(_, _, n)| n).sum();
    2.0 * matches as f64 / total as f64
}

fn matching_blocks(a: &[char], b: &[char]) -> Vec<(usize, usize, usize)> {
    // b2j with the default autojunk heuristic (only for len(b) >= 200).
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    let n = b.len();
    if n >= 200 {
        let ntest = n / 100 + 1;
        b2j.retain(|_, idxs| idxs.len() <= ntest);
    }

    let find_longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ch) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut new: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(ch) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    new.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = new;
        }
        // No junk (isjunk=None): extend over equal neighbours, incl. popular ones.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    };

    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest(alo, ahi, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();
    blocks
}

/// Best case-insensitive fuzzy match with ratio above `threshold` (0.8).
pub fn find_similar_name<'a>(
    target: &str,
    candidates: &[&'a str],
    threshold: f64,
) -> Option<&'a str> {
    let target = target.to_lowercase();
    let mut best = None;
    let mut best_ratio = threshold;
    for &candidate in candidates {
        let ratio = sequence_ratio(&target, &candidate.to_lowercase());
        if ratio > best_ratio {
            best_ratio = ratio;
            best = Some(candidate);
        }
    }
    best
}

// --------------------------------------------------------------- validate

/// Validate sheet pin / hierarchical label consistency across a project.
/// `specific_sheet` restricts checks to nodes whose file name matches.
pub fn validate_hierarchy(root_schematic: &str, specific_sheet: Option<&str>) -> ValidationResult {
    let mut result = ValidationResult::new(root_schematic);
    let root = build_hierarchy(root_schematic);

    for node in root.all_nodes() {
        if let Some(only) = specific_sheet.filter(|s| !s.is_empty()) {
            let file = Path::new(&node.path)
                .file_name()
                .map(|f| f.to_string_lossy());
            if file.as_deref() != Some(only) {
                continue;
            }
        }
        result.sheets_checked += 1;

        for sheet in &node.sheets {
            let Some(child) = node.children.iter().find(|c| c.name == sheet.name) else {
                continue;
            };
            result.pins_checked += sheet.pins.len();
            result.labels_checked += child.hierarchical_label_info.len();

            // Python set iteration order is arbitrary; keep first-seen order.
            let mut seen = HashSet::new();
            let child_label_names: Vec<&str> = child
                .hierarchical_label_info
                .iter()
                .map(|l| l.name.as_str())
                .filter(|n| seen.insert(*n))
                .collect();
            let mut labels_by_name: HashMap<&str, &HierarchicalLabelInfo> = HashMap::new();
            for label in &child.hierarchical_label_info {
                labels_by_name.insert(&label.name, label);
            }

            for pin in &sheet.pins {
                if let Some(label) = labels_by_name.get(pin.name.as_str()) {
                    if !directions_compatible(&pin.direction, &label.shape) {
                        result.issues.push(ValidationIssue {
                            issue_type: ValidationIssueType::DirectionMismatch,
                            sheet_name: sheet.name.clone(),
                            sheet_file: sheet.filename.clone(),
                            parent_sheet_name: node.name.clone(),
                            parent_sheet_file: node.path.clone(),
                            pin_name: Some(pin.name.clone()),
                            label_name: Some(label.name.clone()),
                            pin: Some(pin.clone()),
                            label: Some((*label).clone()),
                            message: format!(
                                "Direction mismatch: pin \"{}\" is {}, but label is {}",
                                pin.name, pin.direction, label.shape
                            ),
                            suggestions: vec![FixSuggestion::new(
                                FixType::FixDirection,
                                format!("Change label shape to '{}'", pin.direction),
                                child.path.clone(),
                            )
                            .with(
                                true,
                                &[
                                    ("new_direction", &pin.direction),
                                    ("label_uuid", &label.uuid),
                                ],
                            )],
                            possible_causes: vec![
                                "Pin direction was changed after label was created".into(),
                                "Copy-paste error when creating the sheet".into(),
                            ],
                        });
                    }
                } else {
                    let similar = find_similar_name(&pin.name, &child_label_names, 0.8);
                    let mut suggestions = Vec::new();
                    let mut causes = Vec::new();
                    if let Some(similar) = similar {
                        suggestions.push(
                            FixSuggestion::new(
                                FixType::RenameLabel,
                                format!("Rename label \"{similar}\" to \"{}\"", pin.name),
                                child.path.clone(),
                            )
                            .with(false, &[("old_name", similar), ("new_name", &pin.name)]),
                        );
                        causes.push(format!("Possible typo: found similar label \"{similar}\""));
                    }
                    suggestions.push(
                        FixSuggestion::new(
                            FixType::AddLabel,
                            format!(
                                "Add hierarchical label \"{}\" ({})",
                                pin.name, pin.direction
                            ),
                            child.path.clone(),
                        )
                        .with(false, &[("name", &pin.name), ("direction", &pin.direction)]),
                    );
                    causes.extend(
                        [
                            "Label was deleted from child sheet",
                            "Sheet was replaced with different version",
                            "Pin was added to parent without corresponding label",
                        ]
                        .map(String::from),
                    );
                    result.issues.push(ValidationIssue {
                        issue_type: ValidationIssueType::MissingLabel,
                        sheet_name: sheet.name.clone(),
                        sheet_file: sheet.filename.clone(),
                        parent_sheet_name: node.name.clone(),
                        parent_sheet_file: node.path.clone(),
                        pin_name: Some(pin.name.clone()),
                        label_name: None,
                        pin: Some(pin.clone()),
                        label: None,
                        message: format!(
                            "Sheet pin \"{}\" has no matching hierarchical label in child sheet",
                            pin.name
                        ),
                        suggestions,
                        possible_causes: causes,
                    });
                }
            }

            let mut seen = HashSet::new();
            let pin_names: Vec<&str> = sheet
                .pins
                .iter()
                .map(|p| p.name.as_str())
                .filter(|n| seen.insert(*n))
                .collect();
            for label in &child.hierarchical_label_info {
                if pin_names.contains(&label.name.as_str()) {
                    continue;
                }
                let mut suggestions = Vec::new();
                let mut causes = Vec::new();
                if let Some(similar) = find_similar_name(&label.name, &pin_names, 0.8) {
                    suggestions.push(
                        FixSuggestion::new(
                            FixType::RenamePin,
                            format!("Rename pin \"{similar}\" to \"{}\"", label.name),
                            node.path.clone(),
                        )
                        .with(false, &[("old_name", similar), ("new_name", &label.name)]),
                    );
                    causes.push(format!("Possible typo: found similar pin \"{similar}\""));
                }
                causes.extend(
                    [
                        "Pin was removed from parent sheet",
                        "Label was added without corresponding pin",
                        "Sheet structure was reorganized",
                    ]
                    .map(String::from),
                );
                result.issues.push(ValidationIssue {
                    issue_type: ValidationIssueType::MissingPin,
                    sheet_name: sheet.name.clone(),
                    sheet_file: sheet.filename.clone(),
                    parent_sheet_name: node.name.clone(),
                    parent_sheet_file: node.path.clone(),
                    pin_name: None,
                    label_name: Some(label.name.clone()),
                    pin: None,
                    label: Some(label.clone()),
                    message: format!(
                        "Hierarchical label \"{}\" has no matching pin in parent sheet (orphan label)",
                        label.name
                    ),
                    suggestions,
                    possible_causes: causes,
                });
            }
        }
    }
    result
}

/// Apply an auto-fixable suggestion; returns whether the file changed.
pub fn apply_fix(fix: &FixSuggestion) -> bool {
    if !fix.auto_fixable {
        return false;
    }
    if fix.fix_type == FixType::FixDirection {
        if let (Some(uuid), Some(dir)) = (
            fix.details.get("label_uuid"),
            fix.details.get("new_direction"),
        ) {
            return fix_label_direction(&fix.file_path, uuid, dir);
        }
    }
    false
}

/// Text-level rewrite of a hierarchical label's `(shape ...)` (as upstream:
/// label blocks are delimited by lines, the label is found by UUID).
pub fn fix_label_direction(file_path: &str, label_uuid: &str, new_direction: &str) -> bool {
    let path = Path::new(file_path);
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    let mut in_label = false;
    let mut label_start = 0usize;
    let mut found_uuid = false;
    let mut modified = false;
    let quoted = format!("uuid \"{label_uuid}\"");
    let bare = format!("uuid {label_uuid}");

    for i in 0..lines.len() {
        if lines[i].contains("hierarchical_label") {
            in_label = true;
            label_start = i;
            found_uuid = false;
        } else if in_label {
            if lines[i].contains(&quoted) || lines[i].contains(&bare) {
                found_uuid = true;
            } else if lines[i].trim().starts_with(')') && !lines[i].contains('(') {
                if found_uuid {
                    for line in lines.iter_mut().take(i + 1).skip(label_start) {
                        if line.contains("(shape ") {
                            *line = replace_shape(line, new_direction);
                            modified = true;
                            break;
                        }
                    }
                }
                in_label = false;
            }
        }
    }
    if modified {
        return std::fs::write(path, lines.join("\n")).is_ok();
    }
    false
}

/// `re.sub(r"\(shape \w+\)", "(shape NEW)", line)`.
fn replace_shape(line: &str, new_direction: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(pos) = rest.find("(shape ") {
        let after = &rest[pos + 7..];
        let word_len: usize = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum();
        if word_len > 0 && after[word_len..].starts_with(')') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("(shape {new_direction})"));
            rest = &after[word_len + 1..];
        } else {
            out.push_str(&rest[..pos + 7]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Human-readable validation report (`show_tree` is accepted for parity).
pub fn format_validation_report(result: &ValidationResult, _show_tree: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    let name = Path::new(&result.root_schematic)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    lines.push(format!("Schematic Hierarchy Validation: {name}"));
    lines.push("=".repeat(70));
    lines.push(String::new());
    lines.push(format!("Sheets checked: {}", result.sheets_checked));
    lines.push(format!("Pins checked: {}", result.pins_checked));
    lines.push(format!("Labels checked: {}", result.labels_checked));
    lines.push(String::new());

    if result.issues.is_empty() {
        lines.push("No issues found. Hierarchy is valid.".into());
        return lines.join("\n");
    }

    lines.push("Validation Results:".into());
    lines.push("-".repeat(70));

    let mut by_sheet: OrderedMap<Vec<&ValidationIssue>> = OrderedMap::new();
    for issue in &result.issues {
        let key = format!("{} ({})", issue.sheet_name, issue.sheet_file);
        if !by_sheet.contains_key(&key) {
            by_sheet.insert(key.clone(), Vec::new());
        }
        by_sheet.get_mut(&key).expect("inserted").push(issue);
    }

    for (sheet_key, issues) in by_sheet.iter() {
        lines.push(String::new());
        let errors = issues.iter().filter(|i| i.severity() == "error").count();
        let status = if errors > 0 { "FAIL" } else { "WARN" };
        lines.push(format!("[{status}] Sheet: {sheet_key}"));
        for issue in issues {
            let icon = if issue.severity() == "error" {
                "x"
            } else {
                "!"
            };
            lines.push(format!("  {icon} {}", issue.message));
            if let Some(pin) = &issue.pin {
                lines.push(format!(
                    "    Pin: \"{}\" ({}) @ ({:.2}, {:.2})",
                    pin.name, pin.direction, pin.position.0, pin.position.1
                ));
            }
            if let Some(label) = &issue.label {
                lines.push(format!(
                    "    Label: \"{}\" ({}) @ ({:.2}, {:.2})",
                    label.name, label.shape, label.position.0, label.position.1
                ));
            }
            if !issue.possible_causes.is_empty() {
                lines.push("    Possible causes:".into());
                for cause in issue.possible_causes.iter().take(2) {
                    lines.push(format!("      - {cause}"));
                }
            }
            if !issue.suggestions.is_empty() {
                lines.push("    Suggested fixes:".into());
                for s in &issue.suggestions {
                    let auto = if s.auto_fixable {
                        " [auto-fixable]"
                    } else {
                        ""
                    };
                    lines.push(format!("      - {}{auto}", s.description));
                }
            }
        }
    }

    lines.push(String::new());
    lines.push("-".repeat(70));
    lines.push(format!(
        "Summary: {} error(s), {} warning(s) in {} sheet(s)",
        result.error_count(),
        result.warning_count(),
        by_sheet.len()
    ));
    lines.join("\n")
}
