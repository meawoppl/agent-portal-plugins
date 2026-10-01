//! Python format-spec helpers that `format!` has no direct equivalent for.

/// Python `format(v, ".{precision}g")` (`f"{v:g}"` is precision 6).
pub fn format_g(v: f64, precision: usize) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let p = precision.max(1);
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    // Round to `p` significant digits first; the exponent of the rounded
    // value decides fixed vs scientific notation (CPython semantics).
    let sci = format!("{:.*e}", p - 1, v);
    let (mantissa, exp) = sci.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent int");
    if exp < -4 || exp >= p as i32 {
        let m = strip_zeros(mantissa);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{m}e{sign}{:02}", exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{v:.decimals$}"))
    }
}

/// `f"{v:g}"`.
pub fn g(v: f64) -> String {
    format_g(v, 6)
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_matches_python() {
        assert_eq!(g(1.0), "1");
        assert_eq!(g(0.5), "0.5");
        assert_eq!(g(2.0), "2");
        assert_eq!(g(1e-5), "1e-05");
        assert_eq!(g(123456789.0), "1.23457e+08");
        assert_eq!(g(0.0001), "0.0001");
        assert_eq!(g(100000.0), "100000");
        assert_eq!(g(1000000.0), "1e+06");
        assert_eq!(format_g(0.123456789012, 9), "0.123456789");
        assert_eq!(g(-2.5), "-2.5");
    }
}
