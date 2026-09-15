//! Tokenizer + parser for server.cfg.
//!
//! Grammar (per line):
//!   line     := indent? (comment | directive)? '\n'
//!   comment  := ('#' | '//') until-end-of-line
//!   directive:= verb (ws+ arg)*
//!   arg      := bare | quoted
//!   quoted   := '"' (escape | char)* '"'
//!   escape   := '\' ('"' | '\' | 'n' | 't' | 'r' | other)
//!   bare     := run of non-whitespace, non-'"' chars
//!
//! Errors are located by (line, column). Unknown verbs are accepted by default so
//! custom convars keep working; `ParseOptions::strict_verbs` rejects them for `ald validate`.

use crate::ast::{Directive, Document, Line, LineKind, Value};
use crate::error::{is_known, CfgError, CfgResult};

/// Controls `${VAR}` expansion in argument values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvExpansion {
    /// No expansion at all; `${VAR}` stays literal.
    Disabled,
    /// Expand `${VAR}` from the process environment; an unset name is a hard error.
    Enabled,
}

#[derive(Debug, Clone)]
pub struct ParseOptions {
    pub env: EnvExpansion,
    /// Reject verbs not in the whitelist.
    pub strict_verbs: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        // Spec: unknown custom convars must be supported, so we accept any verb by default.
        // `ald config validate` can opt into strict mode to surface typos in security directives.
        ParseOptions { env: EnvExpansion::Enabled, strict_verbs: false }
    }
}

/// Parse a single source into a document. Include resolution is a separate pass
/// ([`crate::IncludeResolver`]) so parsing never does I/O.
pub fn parse_document(source: &str, text: &str, opts: &ParseOptions) -> CfgResult<Document> {
    let mut doc = Document::new(source);
    let segs: Vec<&str> = text.split('\n').collect();
    // `"a\n".split('\n')` yields ["a", ""]: the trailing empty segment is the
    // position after the final newline, not an extra line, so it is dropped.
    // Interior empty segments (e.g. "a\n\nb") are real blank lines and stay.
    let last = if segs.len() > 1 && segs.last() == Some(&"") {
        segs.len() - 1
    } else {
        segs.len()
    };
    for (idx, raw) in segs[..last].iter().enumerate() {
        let line_no = (idx as u32) + 1;
        doc.push(parse_line(line_no, raw, opts)?);
    }
    if last == 0 {
        // An empty file still has one (blank) line so render() is never empty.
        doc.push(parse_line(1, "", opts)?);
    }
    Ok(doc)
}

fn parse_line(line_no: u32, raw: &str, opts: &ParseOptions) -> CfgResult<Line> {
    let trimmed_start = raw.trim_start();
    let indent_len = raw.len() - trimmed_start.len();
    let indent = raw[..indent_len].to_string();

    // Strip a trailing '\r' (CRLF files) before classification.
    let body = trimmed_start.trim_end_matches('\r');

    if body.is_empty() {
        return Ok(Line { line_no, indent, kind: LineKind::Blank });
    }
    // The comment marker is preserved verbatim so `Document::render` round-trips exactly.
    if body.starts_with('#') || body.starts_with("//") {
        return Ok(Line { line_no, indent, kind: LineKind::Comment(body.to_string()) });
    }

    let mut toks = Tokenizer::new(body, line_no);
    let verb = match toks.next_bare()? {
        Some(v) => v,
        None => return Ok(Line { line_no, indent, kind: LineKind::Blank }),
    };

    if opts.strict_verbs && !is_known(&verb) {
        return Err(CfgError::UnknownDirective {
            line: line_no,
            verb: verb.clone(),
            allowed: crate::error::KNOWN_DIRECTIVES.join(", "),
        });
    }

    let mut args = Vec::new();
    let mut raw_arg_count = 0usize;
    while let Some(tok) = toks.next_token()? {
        raw_arg_count += 1;
        let v = match tok {
            Token::Bare(s) => Value::Bare(expand_env(&s, opts, line_no)?),
            Token::Quoted(s) => Value::Quoted(expand_env(&s, opts, line_no)?),
        };
        args.push(v);
    }

    Ok(Line { line_no, indent, kind: LineKind::Directive(Directive { verb, args, raw_arg_count }) })
}

enum Token {
    Bare(String),
    Quoted(String),
}

struct Tokenizer<'a> {
    s: &'a [u8],
    pos: usize,
    line: u32,
}

impl<'a> Tokenizer<'a> {
    fn new(s: &'a str, line: u32) -> Self {
        Tokenizer { s: s.as_bytes(), pos: 0, line }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.s.len() && (self.s[self.pos] as char).is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Read one whitespace-delimited bare token (used for the verb).
    fn next_bare(&mut self) -> CfgResult<Option<String>> {
        self.skip_ws();
        if self.pos >= self.s.len() {
            return Ok(None);
        }
        let start = self.pos;
        while self.pos < self.s.len() {
            let c = self.s[self.pos] as char;
            if c.is_ascii_whitespace() || c == '"' {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            // Sitting on a quote: a verb cannot be quoted.
            return Err(CfgError::InvalidValue {
                line: self.line,
                reason: "directive verb must be an unquoted token".into(),
            });
        }
        Ok(Some(String::from_utf8_lossy(&self.s[start..self.pos]).into_owned()))
    }

    fn next_token(&mut self) -> CfgResult<Option<Token>> {
        self.skip_ws();
        if self.pos >= self.s.len() {
            return Ok(None);
        }
        if self.s[self.pos] == b'"' {
            Ok(Some(Token::Quoted(self.read_quoted()?)))
        } else {
            let start = self.pos;
            while self.pos < self.s.len() {
                let c = self.s[self.pos] as char;
                if c.is_ascii_whitespace() {
                    break;
                }
                self.pos += 1;
            }
            Ok(Some(Token::Bare(String::from_utf8_lossy(&self.s[start..self.pos]).into_owned())))
        }
    }

    fn read_quoted(&mut self) -> CfgResult<String> {
        debug_assert_eq!(self.s[self.pos], b'"');
        let col = self.pos as u32 + 1;
        self.pos += 1; // consume opening quote
        let mut out = String::new();
        while self.pos < self.s.len() {
            let c = self.s[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.s.get(self.pos).ok_or(CfgError::UnterminatedString {
                        line: self.line,
                        col,
                    })?;
                    // Recognized escapes decode; anything else passes through verbatim
                    // (FiveM-style leniency: `\c` yields `c`).
                    let ch = match esc {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        other => *other as char,
                    };
                    out.push(ch);
                    self.pos += 1;
                }
                _ => {
                    out.push(c as char);
                    self.pos += 1;
                }
            }
        }
        Err(CfgError::UnterminatedString { line: self.line, col })
    }
}

fn expand_env(s: &str, opts: &ParseOptions, line: u32) -> CfgResult<String> {
    if opts.env == EnvExpansion::Disabled || !s.contains("${") {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let name = &s[i + 2..i + 2 + end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    // An unset var silently becoming an empty string would hide typos in
                    // secret references (`set_secret db_url env:TYPO`) — fail loudly instead.
                    Err(_) => {
                        return Err(CfgError::UnknownEnvVar { line, name: name.to_string() })
                    }
                }
                i = i + 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> CfgResult<Document> {
        parse_document("test.cfg", text, &ParseOptions::default())
    }

    // `line` is 1-based, matching server.cfg line numbers.
    fn directive(doc: &Document, line: u32) -> &Directive {
        for d in doc.directives() {
            if d.0 == line {
                return d.1;
            }
        }
        panic!("no directive on line {line}");
    }

    #[test]
    fn parses_quoted_hostname() {
        let d = parse(r#"sv_hostname "My Server""#).unwrap();
        assert_eq!(directive(&d, 1).first_arg(), Some("My Server"));
    }

    #[test]
    fn parses_bare_int() {
        let d = parse("sv_maxclients 48").unwrap();
        assert_eq!(directive(&d, 1).first_arg(), Some("48"));
    }

    #[test]
    fn handles_escapes() {
        // Source:  sv_hostname "a\"b\\c"   -> value  a"b\c
        // (\" is a literal quote; \\ is a literal backslash; \c is unknown -> c)
        let d = parse(r#"sv_hostname "a\"b\\c""#).unwrap();
        assert_eq!(directive(&d, 1).first_arg(), Some("a\"b\\c"));
    }

    #[test]
    fn unknown_escape_passes_through() {
        let d = parse(r#"sv_hostname "a\cb""#).unwrap();
        assert_eq!(directive(&d, 1).first_arg(), Some("acb"));
    }

    #[test]
    fn rejects_unterminated_string() {
        assert!(parse(r#"sv_hostname "My Server"#).is_err());
    }

    #[test]
    fn quoted_verb_rejected() {
        assert!(parse(r#""sv_hostname" x"#).is_err());
    }

    #[test]
    fn unknown_directives_allowed_by_default() {
        // Spec: unknown custom convars must be supported.
        assert!(parse("my_custom_convar 1").is_ok());
        let strict = ParseOptions { strict_verbs: true, ..ParseOptions::default() };
        assert!(parse_document("t.cfg", "my_custom_convar 1", &strict).is_err());
    }

    #[test]
    fn comments_and_blanks_preserved_and_roundtrip() {
        // 5 lines: comment, blank, directive, blank, comment. The trailing newline
        // terminates line 5 and does not create a 6th line.
        let src = "# header comment\n\n  sv_hostname \"x\"\n   \n// tail\n";
        let d = parse(src).unwrap();
        assert_eq!(d.len(), 5);
        assert!(matches!(d.lines[0].kind, LineKind::Comment(_)));
        assert!(matches!(d.lines[1].kind, LineKind::Blank));
        assert!(matches!(d.lines[2].kind, LineKind::Directive(_)));
        assert_eq!(d.lines[2].indent, "  ");
        assert!(matches!(d.lines[3].kind, LineKind::Blank));
        assert!(matches!(d.lines[4].kind, LineKind::Comment(_)));
        assert_eq!(d.render(), src);
    }

    #[test]
    fn interior_blank_lines_preserved() {
        // Two real blank lines in the middle, plus a trailing newline.
        let d = parse("a 1\n\n\nb 2\n").unwrap();
        assert_eq!(d.len(), 4);
        assert!(matches!(d.lines[1].kind, LineKind::Blank));
        assert!(matches!(d.lines[2].kind, LineKind::Blank));
    }

    #[test]
    fn crlf_line_endings() {
        // Two CRLF-terminated lines; the trailing \r\n does not add a third line.
        let d = parse("sv_maxclients 48\r\nsv_hostname \"x\"\r\n").unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(directive(&d, 1).first_arg(), Some("48"));
        assert_eq!(directive(&d, 2).first_arg(), Some("x"));
    }

    #[test]
    fn empty_document_is_one_blank_line() {
        let d = parse("").unwrap();
        assert_eq!(d.len(), 1);
        assert!(matches!(d.lines[0].kind, LineKind::Blank));
    }

    #[test]
    fn multiple_args() {
        let d = parse("add_ace group.admin command allow").unwrap();
        let args: Vec<&str> = directive(&d, 1).args.iter().map(|v| v.as_str()).collect();
        assert_eq!(args, vec!["group.admin", "command", "allow"]);
    }

    #[test]
    fn unknown_env_var_errors() {
        std::env::remove_var("ALDIVINE_TEST_NOPE");
        let e = parse(r#"sv_hostname "${ALDIVINE_TEST_NOPE}""#).unwrap_err();
        assert!(matches!(e, CfgError::UnknownEnvVar { name, .. } if name == "ALDIVINE_TEST_NOPE"));
    }

    #[test]
    fn env_expansion_disabled_keeps_literal() {
        std::env::set_var("ALD_TEST_LITERAL", "expanded");
        let opts = ParseOptions { env: EnvExpansion::Disabled, ..ParseOptions::default() };
        let d = parse_document("t.cfg", r#"sv_hostname "${ALD_TEST_LITERAL}""#, &opts).unwrap();
        assert_eq!(directive(&d, 1).first_arg(), Some("${ALD_TEST_LITERAL}"));
        std::env::remove_var("ALD_TEST_LITERAL");
    }

    #[test]
    fn env_expansion_enabled_substitutes() {
        std::env::set_var("ALD_TEST_EXPAND", "substituted");
        let d = parse(r#"sv_hostname "${ALD_TEST_EXPAND}""#).unwrap();
        assert_eq!(directive(&d, 1).first_arg(), Some("substituted"));
        std::env::remove_var("ALD_TEST_EXPAND");
    }
}
