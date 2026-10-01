//! Python-compatible `repr()` / `str()` formatting.
//!
//! Upstream messages interpolate values with f-strings (`{x}` / `{x!r}`);
//! these helpers reproduce CPython's output so ported messages match.

use serde_json::Value;

/// CPython `repr(str)`: single quotes unless the text contains `'` but no
/// `"`, with backslash escapes for quotes, backslashes, and non-printables.
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..=0xa0).contains(&(c as u32)) || c == '\u{ad}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if is_unprintable(c) => {
                let v = c as u32;
                if v <= 0xffff {
                    out.push_str(&format!("\\u{v:04x}"));
                } else {
                    out.push_str(&format!("\\U{v:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Approximation of `str.isprintable()` for non-ASCII code points: line and
/// paragraph separators, surrogates/private use, and format characters.
fn is_unprintable(c: char) -> bool {
    matches!(c as u32,
        0x2028 | 0x2029 | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064
        | 0xfeff | 0xe000..=0xf8ff | 0xfff9..=0xfffb | 0xf0000..=0x10ffff)
}

/// CPython `repr(float)`: shortest round-trip digits, `.0` for integral
/// values, exponent notation outside `1e-4 <= |v| < 1e16`.
pub fn py_float_repr(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // `{:e}` yields the shortest round-trip mantissa, e.g. "1.2345e-5".
    let sci = format!("{:e}", v.abs());
    let (mantissa, exp) = sci.split_once('e').expect("scientific format");
    let exp: i32 = exp.parse().expect("exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if v < 0.0 { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let body = if exp >= 0 {
            let int_len = exp + 1;
            if n <= int_len {
                format!("{digits}{}.0", "0".repeat((int_len - n) as usize))
            } else {
                format!(
                    "{}.{}",
                    &digits[..int_len as usize],
                    &digits[int_len as usize..]
                )
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        };
        format!("{sign}{body}")
    } else {
        let mant = if digits.len() > 1 {
            format!("{}.{}", &digits[..1], &digits[1..])
        } else {
            digits
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{mant}e{esign}{:02}", exp.abs())
    }
}

/// CPython `str(x)` for a JSON-shaped value (`str` of a string is the raw
/// text; containers render their items with `repr`).
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// CPython `repr(x)` for a JSON-shaped value.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                py_float_repr(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => py_str_repr(s),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_cpython() {
        assert_eq!(py_float_repr(100.0), "100.0");
        assert_eq!(py_float_repr(0.1), "0.1");
        assert_eq!(py_float_repr(1e-5), "1e-05");
        assert_eq!(py_float_repr(1.5e-5), "1.5e-05");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(1234.5), "1234.5");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(-2.5), "-2.5");
        assert_eq!(py_float_repr(123456789012345.0), "123456789012345.0");
    }

    #[test]
    fn str_repr_matches_cpython() {
        assert_eq!(py_str_repr("abc"), "'abc'");
        assert_eq!(py_str_repr("it's"), "\"it's\"");
        assert_eq!(py_str_repr("a\nb\x1b"), "'a\\nb\\x1b'");
        assert_eq!(py_str_repr("both ' \""), "'both \\' \"'");
    }
}
