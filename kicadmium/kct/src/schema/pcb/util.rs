//! Small helpers shared by the PCB model: Python-`SExp`-style accessors,
//! node construction, UUIDs, glob matching and power-net heuristics.

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::sexp::{SExp, Value};

/// Field that never participates in `==` (Python `field(compare=False)`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Untracked<T>(pub T);

impl<T> PartialEq for Untracked<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

// ------------------------------------------------------------ accessors

/// Python `SExp.get_string(i)`: the atom's text (numbers as written).
pub(crate) fn gs(node: &SExp, index: usize) -> Option<String> {
    let child = node.children.get(index)?;
    if child.is_atom() {
        node.text_at(index)
    } else {
        None
    }
}

/// `get_string(i) or default`.
pub(crate) fn gs_or(node: &SExp, index: usize, default: &str) -> String {
    match gs(node, index) {
        Some(s) if !s.is_empty() => s,
        _ => default.to_string(),
    }
}

/// Python `SExp.get_float(i)`.
pub(crate) fn gf(node: &SExp, index: usize) -> Option<f64> {
    node.float_at(index)
}

/// `get_float(i) or default` (a parsed `0` also falls back, as in Python).
pub(crate) fn gf_or(node: &SExp, index: usize, default: f64) -> f64 {
    match gf(node, index) {
        Some(v) if v != 0.0 => v,
        _ => default,
    }
}

/// Python `SExp.get_int(i)`: ints, or strings that parse as ints.
pub(crate) fn gi(node: &SExp, index: usize) -> Option<i64> {
    match node.value_at(index)? {
        Value::Int(i) => Some(*i),
        Value::Str(s) => s.trim().parse().ok(),
        Value::Float(_) => None,
    }
}

/// `(x y)` pair of a coordinate node, missing values as 0.
pub(crate) fn xy(node: &SExp) -> (f64, f64) {
    (gf(node, 0).unwrap_or(0.0), gf(node, 1).unwrap_or(0.0))
}

/// String-typed atom children (`layers.values[i]` that are `str`).
pub(crate) fn string_atoms(node: &SExp) -> Vec<String> {
    node.children
        .iter()
        .filter_map(|c| match (&c.value, c.is_atom()) {
            (Some(Value::Str(s)), true) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// Whether a direct atom child has string value `token`.
pub(crate) fn has_atom(node: &SExp, token: &str) -> bool {
    node.children
        .iter()
        .any(|c| c.is_atom() && c.value.as_ref().and_then(Value::as_str) == Some(token))
}

/// `(x y)` of a direct-or-descendant coordinate node (`find`).
pub(crate) fn find_xy(node: &SExp, tag: &str) -> Option<(f64, f64)> {
    node.find(tag).map(xy)
}

// ------------------------------------------------------------ construction

pub(crate) fn list(name: &str, children: Vec<SExp>) -> SExp {
    SExp::list(name, children)
}

pub(crate) fn atom(value: impl Into<Value>) -> SExp {
    SExp::atom(value)
}

pub(crate) fn pair_xy(name: &str, x: f64, y: f64) -> SExp {
    list(name, vec![atom(x), atom(y)])
}

// --------------------------------------------------------------- tree paths

/// Child-index paths of every descendant named `tag`, in pre-order (the
/// order Python `find_all` yields them).
pub(crate) fn descendant_paths(node: &SExp, tag: &str) -> Vec<Vec<usize>> {
    fn walk(node: &SExp, tag: &str, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            if child.has_tag(tag) {
                out.push(path.clone());
            }
            walk(child, tag, path, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(node, tag, &mut Vec::new(), &mut out);
    out
}

pub(crate) fn at_path_mut<'a>(node: &'a mut SExp, path: &[usize]) -> Option<&'a mut SExp> {
    let mut cur = node;
    for &i in path {
        cur = cur.children.get_mut(i)?;
    }
    Some(cur)
}

/// Visit every descendant named `tag` mutably (pre-order).
pub(crate) fn for_each_named_mut(node: &mut SExp, tag: &str, f: &mut dyn FnMut(&mut SExp)) {
    for path in descendant_paths(node, tag) {
        if let Some(n) = at_path_mut(node, &path) {
            f(n);
        }
    }
}

// -------------------------------------------------------------------- uuid

static UUID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn random_u64(salt: u64) -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(salt);
    h.write_u64(UUID_COUNTER.fetch_add(1, Ordering::Relaxed));
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    h.write_u32(std::process::id());
    h.finish()
}

/// Random RFC 4122 version-4 UUID string (Python `str(uuid.uuid4())`).
pub fn new_uuid() -> String {
    let hi = random_u64(0x9e37_79b9_7f4a_7c15);
    let lo = random_u64(0xd1b5_4a32_d192_ed03);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// -------------------------------------------------------------------- glob

/// Python `fnmatch.fnmatch` (POSIX: case-sensitive): `*`, `?`, `[seq]`,
/// `[!seq]`.
pub fn fnmatch(name: &str, pattern: &str) -> bool {
    fn class(p: &[char], c: char) -> Option<(bool, usize)> {
        // p[0] == '['; returns (matched, consumed length) or None if unterminated.
        let mut i = 1;
        let negate = p.get(1) == Some(&'!');
        if negate {
            i += 1;
        }
        let start = i;
        let mut matched = false;
        while i < p.len() {
            if p[i] == ']' && i > start {
                return Some((matched != negate, i + 1));
            }
            if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                if p[i] <= c && c <= p[i + 2] {
                    matched = true;
                }
                i += 3;
            } else {
                if p[i] == c {
                    matched = true;
                }
                i += 1;
            }
        }
        None
    }
    fn rec(n: &[char], p: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|k| rec(&n[k..], &p[1..])),
            Some('?') => !n.is_empty() && rec(&n[1..], &p[1..]),
            Some('[') => match (n.first(), class(p, n.first().copied().unwrap_or('\0'))) {
                (Some(_), Some((ok, len))) => ok && rec(&n[1..], &p[len..]),
                (None, _) => false,
                (Some(&c), None) => c == '[' && rec(&n[1..], &p[1..]),
            },
            Some(&c) => n.first() == Some(&c) && rec(&n[1..], &p[1..]),
        }
    }
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    rec(&n, &p)
}

// --------------------------------------------------------------- power nets

/// Built-in power/ground heuristic, Python
/// `^(\+|GND|GNDA|GNDPWR|VCC|VDD|VBUS|VSS|V[0-9])` (case-insensitive).
pub fn is_power_net_default(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if ["+", "GND", "VCC", "VDD", "VBUS", "VSS"]
        .iter()
        .any(|p| upper.starts_with(p))
    {
        return true;
    }
    let b = upper.as_bytes();
    b.len() >= 2 && b[0] == b'V' && b[1].is_ascii_digit()
}

/// Python `_is_power_net`: custom predicate when given, else the built-in
/// heuristic. Empty names are never power nets.
pub fn is_power_net(name: &str, pattern: Option<&dyn Fn(&str) -> bool>) -> bool {
    if name.is_empty() {
        return false;
    }
    match pattern {
        Some(p) => p(name),
        None => is_power_net_default(name),
    }
}

fn split_sign_digits(name: &str) -> Option<(char, &str)> {
    let mut chars = name.chars();
    let sign = chars.next()?;
    if sign != '+' && sign != '-' {
        return None;
    }
    Some((sign, &name[1..]))
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Canonical power-rail spelling: `+3V3`/`+3.3V` -> `+3.3V`, `+5.0V` ->
/// `+5V`. Non-rail names are returned unchanged.
pub fn canonicalize_power_net(name: &str) -> String {
    let Some((sign, rest)) = split_sign_digits(name) else {
        return name.to_string();
    };
    let fmt = |whole: &str, frac: &str| {
        if frac == "0" {
            format!("{sign}{whole}V")
        } else {
            format!("{sign}{whole}.{frac}V")
        }
    };
    // +3V3
    if let Some((whole, frac)) = rest.split_once('V') {
        if all_digits(whole) && all_digits(frac) {
            return fmt(whole, frac);
        }
    }
    // +3.3V
    if let Some(body) = rest.strip_suffix('V') {
        if let Some((whole, frac)) = body.split_once('.') {
            if all_digits(whole) && all_digits(frac) {
                return fmt(whole, frac);
            }
        }
        // +5V
        if all_digits(body) {
            return format!("{sign}{body}V");
        }
    }
    name.to_string()
}

/// Apply [`canonicalize_power_net`] to every name.
pub fn canonicalize_power_nets<'a, I: IntoIterator<Item = &'a str>>(
    names: I,
) -> std::collections::BTreeSet<String> {
    names.into_iter().map(canonicalize_power_net).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globbing() {
        assert!(fnmatch("C12", "C*"));
        assert!(!fnmatch("R1", "C*"));
        assert!(fnmatch("U1", "U?"));
        assert!(!fnmatch("U10", "U?"));
        assert!(fnmatch("R5", "[RC]*"));
        assert!(!fnmatch("U5", "[!U]*"));
        assert!(fnmatch("a-b", "a[-x]b"));
    }

    #[test]
    fn power_names() {
        for n in ["GND", "+3V3", "+5V", "VCC", "vdd", "VBUS", "V5", "GNDA"] {
            assert!(is_power_net(n, None), "{n}");
        }
        for n in ["AUDIO", "SIG_A", "", "VREF"] {
            assert!(!is_power_net(n, None), "{n}");
        }
    }

    #[test]
    fn canonical_rails() {
        assert_eq!(canonicalize_power_net("+3V3"), "+3.3V");
        assert_eq!(canonicalize_power_net("+3.3V"), "+3.3V");
        assert_eq!(canonicalize_power_net("+1V8"), "+1.8V");
        assert_eq!(canonicalize_power_net("+5V"), "+5V");
        assert_eq!(canonicalize_power_net("+5.0V"), "+5V");
        assert_eq!(canonicalize_power_net("+3V0"), "+3V");
        assert_eq!(canonicalize_power_net("-3V3"), "-3.3V");
        assert_eq!(canonicalize_power_net("VBUS"), "VBUS");
        assert_eq!(canonicalize_power_net("BOOT0"), "BOOT0");
    }

    #[test]
    fn uuids_are_unique_v4() {
        let a = new_uuid();
        let b = new_uuid();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
    }
}
