//! Error hierarchy (port of `kicad_tools.exceptions`).
//!
//! Python's class hierarchy (`KiCadToolsError` with `ParseError`,
//! `ValidationError`, ...) maps to one [`KiCadToolsError`] struct carrying an
//! [`ErrorKind`]; "catch by base class" becomes "downcast to
//! `KiCadToolsError`", and `isinstance(e, FileFormatError)` becomes
//! `e.kind == ErrorKind::FileFormat`. Error codes, message layout, and
//! `to_dict()` JSON match upstream.
//!
//! Also defines [`ValueError`] and [`PermissionError`], the Python builtin
//! exceptions upstream raises from helpers, so callers can distinguish them.

use std::fmt;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::utils::pyrepr::{py_float_repr, py_str, py_str_repr};

/// Python `ValueError` analogue: invalid argument or malformed input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError(pub String);

impl ValueError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

/// Python `PermissionError` analogue (e.g. KiCad lock policy `error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionError(pub String);

impl fmt::Display for PermissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PermissionError {}

/// Position within a KiCad file for precise error reporting.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourcePosition {
    pub file_path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub element_type: String,
    pub element_ref: String,
    pub position_mm: Option<(f64, f64)>,
    pub layer: Option<String>,
}

impl SourcePosition {
    pub fn new(file_path: impl Into<PathBuf>, line: u32, column: u32) -> Self {
        Self {
            file_path: file_path.into(),
            line,
            column,
            ..Default::default()
        }
    }

    /// Python `repr()`.
    pub fn repr(&self) -> String {
        let mut parts = vec![
            format!(
                "file_path={}",
                py_path_repr(&self.file_path.to_string_lossy())
            ),
            format!("line={}", self.line),
            format!("column={}", self.column),
        ];
        if !self.element_type.is_empty() {
            parts.push(format!("element_type={}", py_str_repr(&self.element_type)));
        }
        if !self.element_ref.is_empty() {
            parts.push(format!("element_ref={}", py_str_repr(&self.element_ref)));
        }
        if let Some((x, y)) = self.position_mm {
            parts.push(format!(
                "position_mm=({}, {})",
                py_float_repr(x),
                py_float_repr(y)
            ));
        }
        if let Some(layer) = self.layer.as_deref().filter(|l| !l.is_empty()) {
            parts.push(format!("layer={}", py_str_repr(layer)));
        }
        format!("SourcePosition({})", parts.join(", "))
    }

    /// JSON-serializable form (empty optional fields omitted).
    pub fn to_dict(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "file_path".into(),
            Value::String(self.file_path.to_string_lossy().into_owned()),
        );
        out.insert("line".into(), json!(self.line));
        out.insert("column".into(), json!(self.column));
        if !self.element_type.is_empty() {
            out.insert("element_type".into(), json!(self.element_type));
        }
        if !self.element_ref.is_empty() {
            out.insert("element_ref".into(), json!(self.element_ref));
        }
        if let Some((x, y)) = self.position_mm {
            out.insert("position_mm".into(), json!({"x": x, "y": y}));
        }
        if let Some(layer) = self.layer.as_deref().filter(|l| !l.is_empty()) {
            out.insert("layer".into(), json!(layer));
        }
        Value::Object(out)
    }
}

fn py_path_repr(path: &str) -> String {
    #[cfg(windows)]
    {
        format!("WindowsPath({})", py_str_repr(path))
    }
    #[cfg(not(windows))]
    {
        format!("PosixPath({})", py_str_repr(path))
    }
}

impl fmt::Display for SourcePosition {
    /// `file:line:column` for IDE/editor integration.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.file_path.display(),
            self.line,
            self.column
        )
    }
}

/// CamelCase class name to SCREAMING_SNAKE_CASE error code, dropping an
/// `Error` suffix: `FileNotFoundError` -> `FILE_NOT_FOUND`.
pub fn class_name_to_error_code(class_name: &str) -> String {
    let name = class_name.strip_suffix("Error").unwrap_or(class_name);
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c);
    }
    out.to_uppercase()
}

/// Which upstream exception class an error corresponds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// `KiCadToolsError` itself.
    Base,
    Parse,
    Validation,
    FileFormat,
    FileNotFound,
    Routing,
    Configuration,
    Export,
    KiCadCli,
}

impl ErrorKind {
    /// Upstream Python class name.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Base => "KiCadToolsError",
            Self::Parse => "ParseError",
            Self::Validation => "ValidationError",
            Self::FileFormat => "FileFormatError",
            Self::FileNotFound => "FileNotFoundError",
            Self::Routing => "RoutingError",
            Self::Configuration => "ConfigurationError",
            Self::Export => "ExportError",
            Self::KiCadCli => "KiCadCLIError",
        }
    }

    pub fn default_error_code(self) -> String {
        class_name_to_error_code(self.class_name())
    }
}

/// Base error with context, suggestions, and a machine-readable code.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadToolsError {
    pub kind: ErrorKind,
    pub message: String,
    /// Ordered context entries (Python dict insertion order).
    pub context: Vec<(String, Value)>,
    pub suggestions: Vec<String>,
    pub error_code: String,
    /// Individual messages for `ValidationError`; empty otherwise.
    pub errors: Vec<String>,
}

impl KiCadToolsError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            context: Vec::new(),
            suggestions: Vec::new(),
            error_code: kind.default_error_code(),
            errors: Vec::new(),
        }
    }

    /// `KiCadToolsError(message)`.
    pub fn base(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Base, message)
    }

    pub fn file_format(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::FileFormat, message)
    }

    pub fn file_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::FileNotFound, message)
    }

    pub fn routing(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Routing, message)
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Configuration, message)
    }

    pub fn export(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Export, message)
    }

    pub fn kicad_cli(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::KiCadCli, message)
    }

    /// `ParseError(message, context, line=, column=, file_path=)`: the
    /// convenience fields fill `file`/`line`/`column` unless the explicit
    /// context already has them.
    pub fn parse(
        message: impl Into<String>,
        context: Vec<(String, Value)>,
        line: Option<u32>,
        column: Option<u32>,
        file_path: Option<&str>,
    ) -> Self {
        let mut err = Self::new(ErrorKind::Parse, message);
        err.context = context;
        if let Some(file) = file_path.filter(|f| !f.is_empty()) {
            err.context_default("file", Value::String(file.to_string()));
        }
        if let Some(line) = line {
            err.context_default("line", json!(line));
        }
        if let Some(column) = column {
            err.context_default("column", json!(column));
        }
        err
    }

    /// `ValidationError(errors)`.
    pub fn validation(errors: Vec<String>) -> Self {
        let mut message = format!("Validation failed with {} error(s):\n", errors.len());
        let lines: Vec<String> = errors
            .iter()
            .enumerate()
            .map(|(i, e)| format!("  {}. {e}", i + 1))
            .collect();
        message.push_str(&lines.join("\n"));
        let mut err = Self::new(ErrorKind::Validation, message);
        err.errors = errors;
        err
    }

    fn context_default(&mut self, key: &str, value: Value) {
        if !self.context.iter().any(|(k, _)| k == key) {
            self.context.push((key.to_string(), value));
        }
    }

    /// Append (or replace) a context entry.
    pub fn with_context(mut self, key: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        match self.context.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.context.push((key.to_string(), value)),
        }
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    pub fn with_suggestions<I, S>(mut self, suggestions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.suggestions
            .extend(suggestions.into_iter().map(Into::into));
        self
    }

    pub fn with_error_code(mut self, code: impl Into<String>) -> Self {
        self.error_code = code.into();
        self
    }

    pub fn context_value(&self, key: &str) -> Option<&Value> {
        self.context.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `_format_message()`: message, then `Context:` and `Suggestions:` blocks.
    pub fn format_message(&self) -> String {
        let mut out = self.message.clone();
        if !self.context.is_empty() {
            out.push_str("\n\nContext:");
            for (key, value) in &self.context {
                out.push_str(&format!("\n  {key}: {}", py_str(value)));
            }
        }
        if !self.suggestions.is_empty() {
            out.push_str("\n\nSuggestions:");
            for s in &self.suggestions {
                out.push_str(&format!("\n  - {s}"));
            }
        }
        out
    }

    /// JSON form: `error_code`, `message`, `context`, `suggestions` (plus
    /// `errors` for validation errors).
    pub fn to_dict(&self) -> Value {
        let mut out = Map::new();
        out.insert("error_code".into(), json!(self.error_code));
        out.insert("message".into(), json!(self.message));
        out.insert(
            "context".into(),
            Value::Object(self.context.iter().cloned().collect()),
        );
        out.insert("suggestions".into(), json!(self.suggestions));
        if self.kind == ErrorKind::Validation {
            out.insert("errors".into(), json!(self.errors));
        }
        Value::Object(out)
    }

    /// Terminal rendering in the layout of upstream's `__rich_console__`
    /// (header with code, context panel, source snippet, bulleted
    /// suggestions), as plain text.
    pub fn render(&self) -> String {
        let mut out = Vec::new();
        if self.kind == ErrorKind::Validation {
            out.push(format!(
                "[{}] Validation failed with {} error(s)",
                self.error_code,
                self.errors.len()
            ));
            if let Some(file) = self.context_value("file") {
                out.push(format!("  File: {}", py_str(file)));
            }
            out.push(String::new());
            let body: Vec<String> = self
                .errors
                .iter()
                .enumerate()
                .map(|(i, e)| format!("{}. {e}", i + 1))
                .collect();
            out.push(panel("Errors", &body));
        } else {
            out.push(format!("[{}] {}", self.error_code, self.message));
            let body: Vec<String> = self
                .context
                .iter()
                .filter(|(k, _)| k != "source_snippet" && k != "highlight_line")
                .map(|(k, v)| format!("{k}: {}", py_str(v)))
                .collect();
            if !body.is_empty() {
                out.push(String::new());
                out.push(panel("Context", &body));
            }
            if let Some(snippet) = self.context_value("source_snippet").and_then(Value::as_str) {
                let line = self
                    .context_value("line")
                    .and_then(Value::as_i64)
                    .unwrap_or(1);
                let highlight = self.context_value("highlight_line").and_then(Value::as_i64);
                let file = self
                    .context_value("file")
                    .map(py_str)
                    .unwrap_or_else(|| "source".into());
                let start = (line - 2).max(1);
                let body: Vec<String> = snippet
                    .lines()
                    .enumerate()
                    .map(|(i, text)| {
                        let n = start + i as i64;
                        let mark = if Some(n) == highlight { ">" } else { " " };
                        format!("{mark}{n:>4} {text}")
                    })
                    .collect();
                out.push(String::new());
                out.push(panel(&format!("{file}:{line}"), &body));
            }
        }
        if !self.suggestions.is_empty() {
            out.push(String::new());
            out.push("Suggestions:".into());
            for s in &self.suggestions {
                out.push(format!("  \u{2022} {s}"));
            }
        }
        out.join("\n")
    }
}

fn panel(title: &str, lines: &[String]) -> String {
    let width = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 2);
    let mut out = format!(
        "\u{256d}\u{2500} {title} {}\u{256e}\n",
        "\u{2500}".repeat(width.saturating_sub(title.chars().count() + 1))
    );
    for line in lines {
        let pad = width - line.chars().count();
        out.push_str(&format!("\u{2502} {line}{} \u{2502}\n", " ".repeat(pad)));
    }
    out.push_str(&format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(width + 2)));
    out
}

impl fmt::Display for KiCadToolsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format_message())
    }
}

impl std::error::Error for KiCadToolsError {}
