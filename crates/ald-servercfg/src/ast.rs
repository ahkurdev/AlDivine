//! AST for server.cfg — lossless enough to round-trip comments and ordering.
//!
//! ponytail: no edit-in-place of the raw text yet; `Document::render` re-renders from
//! tokens, which is sufficient for Aegis config staging. Patch-based edits are a later
//! nicety, not needed for apply/validate/rollback which works on the normalized model.

use serde::{Deserialize, Serialize};

/// One logical line: optional leading whitespace, a directive or a bare comment/blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// 1-based line number within the owning file (span start).
    pub line_no: u32,
    /// Leading whitespace, preserved verbatim for round-trip.
    pub indent: String,
    pub kind: LineKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    /// `# ...` or `// ...` — kept, never interpreted.
    Comment(String),
    Blank,
    Directive(Directive),
}

/// A parsed directive: verb plus raw argument tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directive {
    pub verb: String,
    /// Arguments in source order. Quoted strings are unescaped here; see [`Value`].
    pub args: Vec<Value>,
    /// Original argument count before any normalization (diagnostics only).
    pub raw_arg_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    /// Bare token, e.g. `sv_maxclients` or `48`.
    Bare(String),
    /// `"quoted string"` — stored unescaped.
    Quoted(String),
}

impl Value {
    pub fn as_str(&self) -> &str {
        match self {
            Value::Bare(s) | Value::Quoted(s) => s,
        }
    }

    /// Render back to source form (re-quotes if the value contains whitespace/`"`).
    pub fn render(&self) -> String {
        match self {
            Value::Bare(s) => s.clone(),
            Value::Quoted(s) => format!("\"{}\"", escape(s)),
        }
    }

    pub fn parse_i64(&self) -> Option<i64> {
        self.as_str().trim().parse::<i64>().ok()
    }

    pub fn parse_bool(&self) -> Option<bool> {
        match self.as_str().trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

impl Directive {
    pub fn name(&self) -> &str {
        &self.verb
    }

    /// First argument as a string, if present.
    pub fn first_arg(&self) -> Option<&str> {
        self.args.first().map(|v| v.as_str())
    }

    /// First two arguments as `(key, value)` — the shape of set/setr/sets/add_ace/...
    pub fn kv(&self) -> Option<(&str, &str)> {
        match self.args.as_slice() {
            [k, v, ..] => Some((k.as_str(), v.as_str())),
            _ => None,
        }
    }

    pub fn render(&self) -> String {
        let mut out = self.verb.clone();
        for a in &self.args {
            out.push(' ');
            out.push_str(&a.render());
        }
        out
    }
}

/// A whole parsed file (or the fully-included virtual document).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// Logical origin; the root document is `<root>`.
    pub source: String,
    pub lines: Vec<Line>,
}

impl Document {
    pub fn new(source: impl Into<String>) -> Self {
        Document { source: source.into(), lines: Vec::new() }
    }

    pub fn push(&mut self, line: Line) {
        self.lines.push(line);
    }

    pub fn directives(&self) -> impl Iterator<Item = (u32, &Directive)> {
        self.lines.iter().filter_map(|l| match &l.kind {
            LineKind::Directive(d) => Some((l.line_no, d)),
            _ => None,
        })
    }

    /// Render back to text. Comments/blank lines/indent preserved.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            out.push_str(&l.indent);
            match &l.kind {
                LineKind::Comment(c) => {
                    out.push_str(c);
                    out.push('\n');
                }
                LineKind::Blank => out.push('\n'),
                LineKind::Directive(d) => {
                    out.push_str(&d.render());
                    out.push('\n');
                }
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}
