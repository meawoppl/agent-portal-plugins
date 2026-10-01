//! KiCad 8/9 net format compatibility (port of `kicad_tools.drc.net_compat`).
//!
//! KiCad 8 writes integer net ids (`(net 5)`); KiCad 9 writes names
//! (`(net "GND")`).

use std::collections::HashMap;

/// Resolve a net atom that may be an integer id or a name into
/// `(net_num, net_name)`; `(0, "")` when it cannot be resolved.
pub fn resolve_net_atom(
    atom: Option<&str>,
    nets: Option<&HashMap<i64, String>>,
    net_names: Option<&HashMap<String, i64>>,
) -> (i64, String) {
    let Some(atom) = atom.filter(|a| !a.is_empty()) else {
        return (0, String::new());
    };
    if let Some(num) = parse_py_int(atom) {
        let name = nets.and_then(|m| m.get(&num)).cloned().unwrap_or_default();
        return (num, name);
    }
    let num = net_names.and_then(|m| m.get(atom)).copied().unwrap_or(0);
    (num, atom.to_string())
}

/// Python `int(str)`: surrounding whitespace, optional sign, digits with
/// single underscores between them.
fn parse_py_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let (sign, digits) = match t.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    digits
        .replace('_', "")
        .parse::<i64>()
        .ok()
        .map(|n| sign * n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_both_formats() {
        let nets: HashMap<i64, String> = [(5, "GND".to_string())].into();
        let names: HashMap<String, i64> = [("GND".to_string(), 5)].into();
        assert_eq!(
            resolve_net_atom(Some("5"), Some(&nets), None),
            (5, "GND".into())
        );
        assert_eq!(
            resolve_net_atom(Some("GND"), None, Some(&names)),
            (5, "GND".into())
        );
        assert_eq!(
            resolve_net_atom(Some("VCC"), None, Some(&names)),
            (0, "VCC".into())
        );
        assert_eq!(resolve_net_atom(None, None, None), (0, String::new()));
        assert_eq!(resolve_net_atom(Some(""), None, None), (0, String::new()));
        assert_eq!(resolve_net_atom(Some("7"), None, None), (7, String::new()));
    }
}
