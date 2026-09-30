//! KiCad-style serializer (port of `SExp.to_string`).

use super::{SExp, Token, Value};

const STRUCTURAL: &[&str] = &[
    "kicad_sch", "kicad_pcb", "lib_symbols", "symbol", "footprint", "title_block", "sheet",
    "sheet_instances", "instances", "project", "effects", "font", "property", "wire", "junction",
    "label", "hierarchical_label", "stroke", "general", "layers", "layer", "stackup", "setup",
    "pcbplotparams", "net", "gr_rect", "gr_circle", "gr_line", "gr_text", "zone", "segment", "via",
    "pad", "fp_text", "fp_line", "fp_circle",
];
const FORCE_EXTRA: &[&str] = &["gr_arc", "gr_poly"];
const NEVER_EXTRA: &[&str] = &[
    "path", "pin", "rectangle", "fill", "polyline", "arc", "circle", "text", "global_label",
    "no_connect",
];
const INLINE_NAMES: &[&str] = &[
    "xy", "at", "size", "stroke", "width", "type", "color", "diameter", "length", "thickness",
    "hide", "name", "number", "uuid", "justify", "start", "end", "mid", "pts", "exclude_from_sim",
    "in_bom", "on_board", "dnp", "fields_autoplaced", "pin_numbers", "pin_names", "offset",
];
const UNQUOTED: &[&str] = &[
    "yes", "no", "true", "false", "hide", "show", "none", "outline", "background", "solid",
    "default", "dash", "dash_dot", "dash_dot_dot", "dot", "left", "right", "center", "top",
    "bottom", "mirror", "front", "back", "input", "output", "bidirectional", "tri_state",
    "passive", "free", "unspecified", "power_in", "power_out", "open_collector", "open_emitter",
    "no_connect", "line", "inverted", "clock", "inverted_clock", "input_low", "clock_low",
    "output_low", "edge_clock_high", "non_logic", "signal", "power", "user", "mixed", "jumper",
    "thru_hole", "smd", "connect", "np_thru_hole", "rect", "oval", "circle", "roundrect",
    "trapezoid", "custom", "top_left", "top_right", "bottom_left", "bottom_right", "reference",
    "value", "thermal_reliefs", "full", "thru_hole_only", "hatch", "hatched", "edge", "blind",
    "micro", "through", "arc", "start", "mid", "end", "italic", "bold", "through_hole", "virtual",
    "exclude_from_pos_files", "exclude_from_bom", "board_only", "dnp", "clearance", "trace_width",
    "via_dia", "via_drill", "uvia_dia", "uvia_drill", "diff_pair_width", "diff_pair_gap",
    "diff_pair_template", "diff_pair", "positive", "negative", "global", "local", "symbols", "x",
    "y", "xy", "allowed", "not_allowed", "allow_missing_courtyard", "allow_soldermask_bridges",
];

fn named(node: &SExp, set: &[&str]) -> bool {
    node.name.as_deref().is_some_and(|n| set.contains(&n))
}

fn never_inline(node: &SExp) -> bool {
    named(node, STRUCTURAL) || named(node, NEVER_EXTRA)
}

fn force_structured(node: &SExp) -> bool {
    named(node, STRUCTURAL) || named(node, FORCE_EXTRA)
}

/// Python `_must_quote`: cannot be a bare token.
fn must_quote(s: &str) -> bool {
    s.is_empty() || s.contains([' ', '\t', '\n', '\r', '"', '(', ')', '\\'])
}

/// Python `_needs_quoting`: KiCad quotes everything except keywords/numbers.
fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if UNQUOTED.contains(&s) || s.starts_with("0x") || s.starts_with("0X") {
        return false;
    }
    python_float(s).is_none()
}

/// Accepts what Python's `float()` accepts for plain tokens.
fn python_float(s: &str) -> Option<f64> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    if matches!(body, "inf" | "infinity" | "nan") {
        return Some(0.0);
    }
    t.replace('_', "").parse::<f64>().ok().filter(|_| !t.is_empty())
}

/// Python `f"{v:.6g}"` with integral floats printed as ints.
pub(crate) fn float(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e16 {
        return format!("{}", v as i64);
    }
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let exp = v.abs().log10().floor() as i32;
    if (-4..6).contains(&exp) {
        let decimals = (5 - exp).max(0) as usize;
        let s = format!("{v:.decimals$}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    } else {
        let s = format!("{v:.5e}");
        let (mant, e) = s.split_once('e').unwrap();
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap();
        format!("{mant}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    }
}

fn escape(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn atom(node: &SExp) -> String {
    match (&node.value, &node.token) {
        (None, _) => String::new(),
        (Some(_), Token::Number(raw)) => raw.clone(),
        (Some(Value::Str(s)), token) => {
            let quote = match token {
                Token::Bare if !must_quote(s) => false,
                Token::Quoted => true,
                _ => needs_quoting(s),
            };
            if quote {
                escape(s)
            } else {
                s.clone()
            }
        }
        (Some(Value::Int(i)), _) => i.to_string(),
        (Some(Value::Float(f)), _) => float(*f),
    }
}

/// Length estimate used by `_should_inline` (`_format_value`).
fn value_len(node: &SExp) -> usize {
    match &node.value {
        None => 0,
        Some(Value::Str(s)) => {
            if needs_quoting(s) {
                s.len() + 2
            } else {
                s.len()
            }
        }
        Some(Value::Int(i)) => i.to_string().len(),
        Some(Value::Float(f)) => python_repr(*f).len(),
    }
}

fn python_repr(f: f64) -> String {
    if f == f.trunc() && f.is_finite() {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

fn should_inline(node: &SExp) -> bool {
    if node.children.is_empty() {
        return true;
    }
    if never_inline(node) {
        return false;
    }
    if node.children.iter().all(SExp::is_atom) {
        let total = node.name.as_deref().map_or(0, str::len)
            + node.children.iter().map(|c| value_len(c) + 1).sum::<usize>();
        return total < 60;
    }
    if named(node, INLINE_NAMES) {
        if node.children.iter().all(|c| c.is_atom() || should_inline(c)) {
            return compact(node).len() < 80;
        }
        return false;
    }
    false
}

pub(crate) fn compact(node: &SExp) -> String {
    if node.is_atom() {
        return atom(node);
    }
    let mut parts: Vec<String> = node.name.iter().cloned().collect();
    parts.extend(node.children.iter().map(compact));
    format!("({})", parts.join(" "))
}

pub(crate) fn pretty(node: &SExp, indent: usize, preserve: bool) -> String {
    if preserve {
        if let Some((text, canonical)) = &node.source_text {
            if *canonical == compact(node) {
                return format!("{}{text}", "\t".repeat(indent));
            }
        }
    }
    if node.is_atom() {
        return atom(node);
    }
    if node.name.is_none() && node.children.is_empty() {
        return "()".into();
    }
    if should_inline(node) {
        return compact(node);
    }
    let tabs = "\t".repeat(indent);
    let mut lines = vec![match &node.name {
        Some(name) => format!("{tabs}({name}"),
        None => format!("{tabs}("),
    }];
    let force = force_structured(node);
    let mut started = false;
    for child in &node.children {
        if child.is_atom() {
            if indent == 0 || started {
                lines.push(format!("{tabs}\t{}", compact(child)));
                started = true;
            } else {
                let last = lines.last_mut().unwrap();
                last.push(' ');
                last.push_str(&compact(child));
            }
        } else if should_inline(child) {
            if indent == 0 || force || started {
                lines.push(format!("{tabs}\t{}", compact(child)));
                started = true;
            } else {
                let last = lines.last_mut().unwrap();
                last.push(' ');
                last.push_str(&compact(child));
            }
        } else {
            lines.push(pretty(child, indent + 1, preserve));
            started = true;
        }
    }
    let last = lines.last_mut().unwrap();
    if last.trim_end().ends_with(')') || indent == 0 {
        lines.push(format!("{tabs})"));
    } else {
        last.push(')');
    }
    lines.join("\n")
}
