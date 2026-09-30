//! Tokenizer/parser matching `kicad_tools.sexp.parser.Parser`.

use super::{SExp, Token, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}:{}", self.message, self.line, self.column)
    }
}

impl std::error::Error for ParseError {}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line_starts: Vec<usize>,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_term(b: u8) -> bool {
    is_ws(b) || b == b'(' || b == b')'
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Parser {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            line_starts,
        }
    }

    fn position(&self, pos: usize) -> (u32, u32) {
        let line = self.line_starts.partition_point(|&s| s <= pos);
        let column = pos - self.line_starts[line - 1] + 1;
        (line as u32, column as u32)
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        let (line, column) = self.position(self.pos.min(self.text.len()));
        ParseError {
            message: message.into(),
            line,
            column,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'#' || b == b';' {
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn expr(&mut self) -> Result<SExp, ParseError> {
        self.skip_ws();
        let start = self.pos;
        let Some(&b) = self.bytes.get(self.pos) else {
            return Err(self.error("Unexpected end of input"));
        };
        let mut node = match b {
            b'(' => self.list()?,
            b'"' => SExp {
                value: Some(Value::Str(self.string()?)),
                token: Token::Quoted,
                ..Default::default()
            },
            b')' => return Err(self.error("Unexpected ')'")),
            _ => self.atom()?,
        };
        let (line, column) = self.position(start);
        node.line = line;
        node.column = column;
        if node.has_tag("arc") && node.get("width").is_some() && node.get("layer").is_some() {
            node.source_text = Some((
                self.text[start..self.pos].to_string(),
                node.to_compact_string(),
            ));
        }
        Ok(node)
    }

    fn list(&mut self) -> Result<SExp, ParseError> {
        self.pos += 1;
        self.skip_ws();
        let Some(&b) = self.bytes.get(self.pos) else {
            return Err(self.error("Unexpected end of input in list"));
        };
        if b == b')' {
            self.pos += 1;
            return Ok(SExp::default());
        }
        let mut node = SExp::default();
        if b != b'(' && b != b'"' {
            // First bare token names the list, numbers included (`(0 "F.Cu" signal)`).
            let start = self.pos;
            while self.pos < self.bytes.len() && !is_term(self.bytes[self.pos]) {
                self.pos += 1;
            }
            node.name = Some(self.text[start..self.pos].to_string());
        } else {
            node.children.push(self.expr()?);
        }
        loop {
            self.skip_ws();
            match self.bytes.get(self.pos) {
                None => return Err(self.error("Unexpected end of input, expected ')'")),
                Some(b')') => {
                    self.pos += 1;
                    return Ok(node);
                }
                Some(_) => node.children.push(self.expr()?),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.pos += 1;
        let mut out = String::new();
        let mut run = self.pos;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    out.push_str(&self.text[run..self.pos]);
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.text[run..self.pos]);
                    self.pos += 1;
                    let Some(ch) = self.text[self.pos..].chars().next() else {
                        return Err(self.error("Unexpected end of input in escape sequence"));
                    };
                    out.push(match ch {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                    self.pos += ch.len_utf8();
                    run = self.pos;
                }
                _ => self.pos += 1,
            }
        }
        Err(self.error("Unterminated string"))
    }

    fn atom(&mut self) -> Result<SExp, ParseError> {
        let start = self.pos;
        while self.pos < self.bytes.len() && !is_term(self.bytes[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error("Expected atom"));
        }
        let token = &self.text[start..self.pos];
        let first = token.as_bytes()[0];
        if first.is_ascii_digit() || (first == b'-' && token.len() > 1) {
            let value = if token.contains(['.', 'e', 'E']) {
                token.parse::<f64>().ok().map(Value::Float)
            } else {
                token.parse::<i64>().ok().map(Value::Int)
            };
            if let Some(value) = value {
                return Ok(SExp {
                    value: Some(value),
                    token: Token::Number(token.to_string()),
                    ..Default::default()
                });
            }
        }
        Ok(SExp {
            value: Some(Value::Str(token.to_string())),
            token: Token::Bare,
            ..Default::default()
        })
    }
}

/// Parse exactly one top-level expression.
pub fn parse(text: &str) -> Result<SExp, ParseError> {
    let mut p = Parser::new(text);
    let node = p.expr()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(p.error("Unexpected content after expression"));
    }
    Ok(node)
}

/// Parse every top-level expression (e.g. `.kicad_dru` rule files).
pub fn parse_all(text: &str) -> Result<Vec<SExp>, ParseError> {
    let mut p = Parser::new(text);
    let mut out = Vec::new();
    loop {
        p.skip_ws();
        if p.pos >= p.bytes.len() {
            return Ok(out);
        }
        out.push(p.expr()?);
    }
}
