//! CPython `json.JSONDecodeError` messages.
//!
//! Upstream surfaces `json.loads` failures verbatim (`f"... {e}"`), e.g.
//! `Expecting value: line 1 column 1 (char 0)`. [`json_decode_error`]
//! re-scans text with CPython 3.12's scanner rules and returns that exact
//! message, or `None` when the text is valid JSON.

struct Scan<'a> {
    s: &'a [char],
}

type R<T> = Result<T, (&'static str, String, usize)>;

fn err<T>(msg: &'static str, pos: usize) -> R<T> {
    Err((msg, String::new(), pos))
}

const WS: [char; 4] = [' ', '\t', '\n', '\r'];

impl Scan<'_> {
    fn ws(&self, mut i: usize) -> usize {
        while i < self.s.len() && WS.contains(&self.s[i]) {
            i += 1;
        }
        i
    }

    fn starts(&self, i: usize, lit: &str) -> bool {
        let l: Vec<char> = lit.chars().collect();
        self.s.len() >= i + l.len() && self.s[i..i + l.len()] == l[..]
    }

    /// `scan_once`: `Err(None)` is StopIteration (no value starts here).
    fn value(&self, i: usize) -> Result<usize, Option<(&'static str, String, usize)>> {
        let Some(&c) = self.s.get(i) else {
            return Err(None);
        };
        match c {
            '"' => self.string(i + 1).map_err(Some),
            '{' => self.object(i + 1).map_err(Some),
            '[' => self.array(i + 1).map_err(Some),
            'n' if self.starts(i, "null") => Ok(i + 4),
            't' if self.starts(i, "true") => Ok(i + 4),
            'f' if self.starts(i, "false") => Ok(i + 5),
            'N' if self.starts(i, "NaN") => Ok(i + 3),
            'I' if self.starts(i, "Infinity") => Ok(i + 8),
            '-' if self.starts(i, "-Infinity") => Ok(i + 9),
            _ => self.number(i).ok_or(None),
        }
    }

    fn number(&self, i: usize) -> Option<usize> {
        let s = self.s;
        let mut j = i;
        if s.get(j) == Some(&'-') {
            j += 1;
        }
        match s.get(j) {
            Some('0') => j += 1,
            Some(c) if c.is_ascii_digit() => {
                while s.get(j).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                }
            }
            _ => return None,
        }
        if s.get(j) == Some(&'.') && s.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
            j += 1;
            while s.get(j).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
            }
        }
        if matches!(s.get(j), Some('e' | 'E')) {
            let mut k = j + 1;
            if matches!(s.get(k), Some('+' | '-')) {
                k += 1;
            }
            if s.get(k).is_some_and(|c| c.is_ascii_digit()) {
                while s.get(k).is_some_and(|c| c.is_ascii_digit()) {
                    k += 1;
                }
                j = k;
            }
        }
        Some(j)
    }

    /// `i` is just past the opening quote.
    fn string(&self, i: usize) -> R<usize> {
        let begin = i - 1;
        let s = self.s;
        let mut j = i;
        loop {
            let Some(&c) = s.get(j) else {
                return err("Unterminated string starting at", begin);
            };
            match c {
                '"' => return Ok(j + 1),
                '\\' => {
                    let Some(&e) = s.get(j + 1) else {
                        return err("Unterminated string starting at", begin);
                    };
                    if e == 'u' {
                        let hex: String = s.iter().skip(j + 2).take(4).collect();
                        if hex.chars().count() != 4 || !hex.chars().all(|h| h.is_ascii_hexdigit()) {
                            return err("Invalid \\uXXXX escape", j + 1);
                        }
                        j += 6;
                    } else if "\"\\/bfnrt".contains(e) {
                        j += 2;
                    } else {
                        return err("Invalid \\escape", j);
                    }
                }
                c if (c as u32) < 0x20 => return err("Invalid control character at", j),
                _ => j += 1,
            }
        }
    }

    fn object(&self, i: usize) -> R<usize> {
        let s = self.s;
        let mut end = self.ws(i);
        match s.get(end) {
            Some('"') => {}
            Some('}') => return Ok(end + 1),
            _ => return err("Expecting property name enclosed in double quotes", end),
        }
        end += 1;
        loop {
            end = self.string(end)?;
            if s.get(end) != Some(&':') {
                end = self.ws(end);
                if s.get(end) != Some(&':') {
                    return err("Expecting ':' delimiter", end);
                }
            }
            end = self.ws(end + 1);
            end = match self.value(end) {
                Ok(e) => e,
                Err(None) => return err("Expecting value", end),
                Err(Some(e)) => return Err(e),
            };
            end = self.ws(end);
            let next = s.get(end).copied();
            end += 1;
            match next {
                Some('}') => return Ok(end),
                Some(',') => {}
                _ => return err("Expecting ',' delimiter", end - 1),
            }
            end = self.ws(end);
            let next = s.get(end).copied();
            end += 1;
            if next != Some('"') {
                return err("Expecting property name enclosed in double quotes", end - 1);
            }
        }
    }

    fn array(&self, i: usize) -> R<usize> {
        let s = self.s;
        let mut end = self.ws(i);
        if s.get(end) == Some(&']') {
            return Ok(end + 1);
        }
        loop {
            end = match self.value(end) {
                Ok(e) => e,
                Err(None) => return err("Expecting value", end),
                Err(Some(e)) => return Err(e),
            };
            end = self.ws(end);
            let next = s.get(end).copied();
            end += 1;
            match next {
                Some(']') => return Ok(end),
                Some(',') => {}
                _ => return err("Expecting ',' delimiter", end - 1),
            }
            end = self.ws(end);
        }
    }
}

fn render(chars: &[char], msg: &str, extra: &str, pos: usize) -> String {
    let before = &chars[..pos.min(chars.len())];
    let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
    let colno = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    format!("{msg}{extra}: line {lineno} column {colno} (char {pos})")
}

/// The `str(JSONDecodeError)` CPython's `json.loads(text)` raises, or `None`
/// when `text` parses.
pub fn json_decode_error(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.first() == Some(&'\u{feff}') {
        return Some(render(
            &chars,
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            "",
            0,
        ));
    }
    let sc = Scan { s: &chars };
    let start = sc.ws(0);
    let end = match sc.value(start) {
        Ok(e) => e,
        Err(None) => return Some(render(&chars, "Expecting value", "", start)),
        Err(Some((m, x, p))) => return Some(render(&chars, m, &x, p)),
    };
    let end = sc.ws(end);
    (end != chars.len()).then(|| render(&chars, "Extra data", "", end))
}

#[cfg(test)]
mod tests {
    use super::json_decode_error as e;

    #[test]
    fn cpython_messages() {
        assert_eq!(e("").unwrap(), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(
            e("{\"a\": 1,}").unwrap(),
            "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)"
        );
        assert_eq!(
            e("[1 2]").unwrap(),
            "Expecting ',' delimiter: line 1 column 4 (char 3)"
        );
        assert_eq!(
            e("{\"a\" 1}").unwrap(),
            "Expecting ':' delimiter: line 1 column 6 (char 5)"
        );
        assert_eq!(e("{}\n x").unwrap(), "Extra data: line 2 column 2 (char 4)");
        assert_eq!(
            e("\"abc").unwrap(),
            "Unterminated string starting at: line 1 column 1 (char 0)"
        );
        assert_eq!(
            e("[\"\\q\"]").unwrap(),
            "Invalid \\escape: line 1 column 3 (char 2)"
        );
        assert!(e("{\"a\": [1, 2.5e3, null, true, \"x\"]}").is_none());
    }
}
