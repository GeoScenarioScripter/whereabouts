//! Tokenizer + generic tree parser shared by the WKT1, ESRI and WKT2 interpreters.
//!
//! Grammar (common to OGC 01-009, ESRI WKT and ISO 19162):
//!
//! ```text
//! node  := KEYWORD ( "[" | "(" ) [ value { "," value } ] ( "]" | ")" )
//! value := node | "quoted text" | number | bare-word
//! ```
//!
//! Keywords are matched case-insensitively (stored upper-cased). Quoted text uses `""` as an
//! escaped quote (WKT2). Either closing bracket closes either opening bracket. The parser is
//! iterative-recursive with an explicit depth limit, a node budget and no panics: every failure
//! is a [`CrsError`].

use crate::{CrsError, MAX_DEPTH, MAX_NODES};

/// A WKT keyword node such as `PROJCS["name", …]`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    /// Upper-cased keyword.
    pub keyword: String,
    /// Arguments in order.
    pub args: Vec<Value>,
}

/// One argument of a [`Node`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Node(Node),
    /// Quoted text, with `""` unescaped.
    Text(String),
    /// A number; `raw` keeps the written form (authority codes must not become `4326.0`).
    Number {
        value: f64,
        raw: String,
    },
    /// An unquoted word that is not followed by a bracket (`NORTH`, `ellipsoidal`, …).
    Word(String),
    /// An unquoted ISO 8601 date/time (`2013-01-01`, `1980-01-06T00:00:00.0Z`). It only appears
    /// in usage/temporal metadata, which the interpreters skip.
    Literal(String),
}

impl Node {
    /// The first argument as text (quoted text, or a bare word for sloppy writers).
    pub fn name(&self) -> Option<&str> {
        match self.args.first()? {
            Value::Text(s) | Value::Word(s) => Some(s),
            _ => None,
        }
    }

    /// The argument at `index` as a number.
    pub fn number(&self, index: usize) -> Option<f64> {
        match self.args.get(index)? {
            Value::Number { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// Child nodes in order.
    pub fn children(&self) -> impl Iterator<Item = &Node> {
        self.args.iter().filter_map(|v| match v {
            Value::Node(n) => Some(n),
            _ => None,
        })
    }

    /// The first child node whose keyword is one of `keywords` (upper-case).
    pub fn child(&self, keywords: &[&str]) -> Option<&Node> {
        self.children()
            .find(|n| keywords.contains(&n.keyword.as_str()))
    }

    /// All child nodes whose keyword is one of `keywords` (upper-case).
    pub fn children_named<'a>(
        &'a self,
        keywords: &'a [&'a str],
    ) -> impl Iterator<Item = &'a Node> + 'a {
        self.children()
            .filter(move |n| keywords.contains(&n.keyword.as_str()))
    }

    /// Visits this node and all descendants (depth-first). Depth is bounded by [`MAX_DEPTH`].
    pub fn walk(&self, f: &mut dyn FnMut(&Node)) {
        f(self);
        for child in self.children() {
            child.walk(f);
        }
    }
}

/// Parses `input` into one or more top-level nodes separated by commas. (ESRI writes compound
/// CRSs as `PROJCS[…],VERTCS[…]` without a wrapper.)
pub(crate) fn parse_document(input: &str) -> Result<Vec<Node>, CrsError> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        src: input,
        pos: 0,
        nodes: 0,
    };
    let mut roots = Vec::new();
    p.skip_ws();
    loop {
        let keyword = p.keyword()?;
        roots.push(p.node(keyword, 0)?);
        p.skip_ws();
        match p.peek() {
            None => break,
            Some(b',') => {
                p.pos += 1;
                p.skip_ws();
            }
            Some(_) => return Err(CrsError::syntax(p.pos, "unexpected text after WKT")),
        }
    }
    Ok(roots)
}

struct Parser<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() || b == 0 {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Reads an identifier `[A-Za-z_][A-Za-z0-9_]*`.
    fn keyword(&mut self) -> Result<String, CrsError> {
        let start = self.pos;
        match self.peek() {
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => self.pos += 1,
            Some(_) => return Err(CrsError::syntax(start, "expected a WKT keyword")),
            None => return Err(CrsError::syntax(start, "unexpected end of input")),
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(self.slice(start, self.pos).to_owned())
    }

    fn slice(&self, start: usize, end: usize) -> &str {
        // Only ever called with boundaries at ASCII bytes, so this cannot fail; `get` keeps it
        // panic-free regardless.
        self.src.get(start..end).unwrap_or("")
    }

    /// Parses the bracketed part of a node whose keyword has just been read.
    fn node(&mut self, keyword: String, depth: usize) -> Result<Node, CrsError> {
        self.skip_ws();
        match self.peek() {
            Some(b'[' | b'(') => {}
            Some(_) => {
                return Err(CrsError::syntax(
                    self.pos,
                    format!("expected `[` after `{keyword}`"),
                ));
            }
            None => return Err(CrsError::syntax(self.pos, "unexpected end of input")),
        }
        if depth >= MAX_DEPTH {
            return Err(CrsError::TooDeep {
                offset: self.pos,
                limit: MAX_DEPTH,
            });
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(CrsError::TooManyNodes { limit: MAX_NODES });
        }
        self.pos += 1;
        let mut node = Node {
            keyword: keyword.to_ascii_uppercase(),
            args: Vec::new(),
        };
        self.skip_ws();
        if matches!(self.peek(), Some(b']' | b')')) {
            self.pos += 1;
            return Ok(node);
        }
        loop {
            self.skip_ws();
            let value = self.value(depth)?;
            node.args.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']' | b')') => {
                    self.pos += 1;
                    return Ok(node);
                }
                Some(_) => {
                    return Err(CrsError::syntax(
                        self.pos,
                        format!("expected `,` or `]` in `{}`", node.keyword),
                    ));
                }
                None => {
                    return Err(CrsError::syntax(
                        self.pos,
                        format!("unterminated `{}`", node.keyword),
                    ));
                }
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, CrsError> {
        match self.peek() {
            Some(b'"') => self.text().map(Value::Text),
            Some(b) if b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.') => self.number(),
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => {
                let word = self.keyword()?;
                self.skip_ws();
                if matches!(self.peek(), Some(b'[' | b'(')) {
                    self.node(word, depth + 1).map(Value::Node)
                } else {
                    Ok(Value::Word(word))
                }
            }
            Some(_) => Err(CrsError::syntax(self.pos, "unexpected character")),
            None => Err(CrsError::syntax(self.pos, "unexpected end of input")),
        }
    }

    fn text(&mut self) -> Result<String, CrsError> {
        let start = self.pos;
        self.pos += 1; // opening quote
        let mut out = String::new();
        let mut chunk_start = self.pos;
        loop {
            match self.peek() {
                Some(b'"') => {
                    out.push_str(self.slice(chunk_start, self.pos));
                    self.pos += 1;
                    if self.peek() == Some(b'"') {
                        // `""` is an escaped quote (ISO 19162 §6.3.3).
                        out.push('"');
                        self.pos += 1;
                        chunk_start = self.pos;
                    } else {
                        return Ok(out);
                    }
                }
                Some(_) => self.pos += 1,
                None => return Err(CrsError::syntax(start, "unterminated quoted text")),
            }
        }
    }

    /// A number, or an unquoted ISO 8601 date/time literal (`2013-01-01`,
    /// `1980-01-06T00:00:00.0Z`, as in `TIMEEXTENT[…]` / `TIMEORIGIN[…]`).
    fn number(&mut self) -> Result<Value, CrsError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b':') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = self.slice(start, self.pos);
        match raw.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Value::Number {
                value,
                raw: raw.to_owned(),
            }),
            Err(_) if is_date_time(raw) => Ok(Value::Literal(raw.to_owned())),
            _ => Err(CrsError::syntax(start, format!("invalid number `{raw}`"))),
        }
    }
}

/// ISO 8601 calendar date, optionally with a time: `YYYY-MM[-DD][Thh[:mm[:ss[.f]]][Z|±hh[:mm]]]`.
/// Checked loosely (shape, not ranges): it is only ever skipped.
fn is_date_time(s: &str) -> bool {
    let b = s.as_bytes();
    let date_ok = b.len() >= 7
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit);
    date_ok
        && b[7..]
            .iter()
            .all(|c| c.is_ascii_digit() || matches!(c, b'-' | b':' | b'.' | b'+' | b'T' | b'Z'))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn parses_nested_nodes_and_values() {
        let roots = parse_document(r#"a["x ""q""", 1.5e3, NORTH, b(2)] , C[]"#).unwrap();
        assert_eq!(roots.len(), 2);
        let a = &roots[0];
        assert_eq!(a.keyword, "A");
        assert_eq!(a.name(), Some(r#"x "q""#));
        assert_eq!(a.number(1), Some(1500.0));
        assert_eq!(a.args[2], Value::Word("NORTH".into()));
        assert_eq!(a.child(&["B"]).unwrap().number(0), Some(2.0));
        assert_eq!(roots[1].keyword, "C");
        assert!(roots[1].args.is_empty());
    }

    #[test]
    fn iso_dates_are_literals() {
        let roots = parse_document(
            "TIMEEXTENT[2013-01-01,2013-12-31T23:59:59.5Z],T[1980-01-06T00:00:00+01:00]",
        )
        .unwrap();
        assert_eq!(roots[0].args[0], Value::Literal("2013-01-01".into()));
        assert_eq!(
            roots[0].args[1],
            Value::Literal("2013-12-31T23:59:59.5Z".into())
        );
        assert_eq!(roots[0].number(0), None);
        assert_eq!(
            roots[1].args[0],
            Value::Literal("1980-01-06T00:00:00+01:00".into())
        );
        for bad in ["A[2013-01]x", "A[201-01-01]", "A[2013-01-01Q]", "A[12:30]"] {
            assert!(parse_document(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "", "A", "A[", "A[\"x", "A[1,", "A[1 2]", "A[1]]", "A[1e999]", "A[--]", "[1]", "A[#]",
        ] {
            assert!(parse_document(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn depth_is_bounded() {
        let deep = "A[".repeat(100_000);
        assert!(matches!(
            parse_document(&deep),
            Err(CrsError::TooDeep { .. })
        ));
    }

    #[test]
    fn node_count_is_bounded() {
        let many = format!("A[{}B[]]", "B[],".repeat(MAX_NODES + 1));
        assert!(matches!(
            parse_document(&many),
            Err(CrsError::TooManyNodes { .. })
        ));
    }
}
