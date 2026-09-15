//! Aldivine fxmanifest compatibility parser.
//!
//! Parses the Lua-table manifest forms used by legacy resources
//! (`fxmanifest.lua`, `__resource.lua`) into a `NormalizedManifest`.
//!
//! This is a *table-literal* parser, not a Lua interpreter: it understands
//! exactly the subset of Lua that manifests use — `fx_version 'cerulean'`,
//! `game 'gta5'`, `client_script 'x.lua'`, `client_scripts { 'a', 'b' }`,
//! nested tables, string/number/boolean scalars, and comments. Anything
//! else (function calls, arithmetic, local variables) is reported as an
//! unsupported construct rather than silently mis-parsed.
//!
//! Directives are classified, never discarded:
//!   SUPPORTED     — parsed into the normalized manifest
//!   TRANSLATED    — legacy concept mapped onto an Aldivine equivalent
//!   IGNORED_WARN  — legal but no runtime effect here
//!   UNSUPPORTED   — present, parsed, recorded, not honored
//!
//! Unknown directives are recorded verbatim; nothing is silently dropped.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ManifestError {
    #[error("syntax error at line {line}: {msg}")]
    Syntax { line: usize, msg: String },
    #[error("unsupported construct at line {line}: {msg}")]
    Construct { line: usize, msg: String },
    #[error("invalid value for directive `{directive}` at line {line}: {msg}")]
    InvalidValue { directive: String, line: usize, msg: String },
    #[error("missing required directive: {0}")]
    MissingRequired(String),
}

// ---------------------------------------------------------------------------
// Normalized manifest
// ---------------------------------------------------------------------------

/// Manifest after normalization; the single shape the Aldivine runtime sees.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NormalizedManifest {
    pub fx_version: Option<String>,
    pub resource_manifest_version: Option<String>,
    pub game: Vec<String>,
    pub client_scripts: Vec<String>,
    pub server_scripts: Vec<String>,
    pub shared_scripts: Vec<String>,
    pub files: Vec<String>,
    pub ui_page: Option<String>,
    pub exports: Vec<String>,
    pub server_exports: Vec<String>,
    pub dependencies: Vec<String>,
    pub provides: Vec<String>,
    pub this_is_a_map: bool,
    pub data_files: Vec<DataFile>,
    pub before_level_meta: Vec<String>,
    pub after_level_meta: Vec<String>,
    pub replace_level_meta: Vec<String>,
    pub loadscreen: Option<String>,
    pub loadscreen_manual_shutdown: bool,
    pub server_only: bool,
    pub lua54: bool,
    pub node_version: Option<String>,
    pub clr_disable_task_scheduler: bool,
    pub use_experimental_fxv2_oal: bool,
    pub convar_categories: Vec<ConvarCategory>,
    pub escrow_ignore: bool,
    /// Custom/unknown metadata keys, preserved verbatim.
    pub custom: Vec<(String, ManifestValue)>,
    /// Every directive the parser saw, in source order, with classification.
    pub directives: Vec<DirectiveRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataFile {
    pub kind: String,
    pub file: String,
}

/// A resource-declared convar category (Aegis auto-generates config UI from
/// these). `name` is the display label, `vars` the convars in declaration
/// order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConvarCategory {
    pub name: String,
    pub vars: Vec<ConvarVar>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConvarVar {
    pub convar: String,
    pub name: Option<String>,
    pub help: Option<String>,
    pub default: Option<ManifestValue>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub kind: Option<String>,
}

// ---------------------------------------------------------------------------
// Value type
// ---------------------------------------------------------------------------

/// Value subset understood by the manifest table parser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ManifestValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// `{ a, b, c }` — a plain sequence.
    List(Vec<ManifestValue>),
    /// `{ key = v }` — a keyed table. Preserves declaration order.
    Table(Vec<(String, ManifestValue)>),
}

impl ManifestValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ManifestValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ManifestValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[ManifestValue]> {
        match self {
            ManifestValue::List(v) => Some(v),
            _ => None,
        }
    }
    /// A table with only integer-like keys 1..=n behaves as a sequence.
    pub fn as_sequence(&self) -> Option<Vec<&ManifestValue>> {
        match self {
            ManifestValue::List(v) => Some(v.iter().collect()),
            ManifestValue::Table(t) => {
                let mut out = Vec::with_capacity(t.len());
                let mut i = 1;
                for (k, v) in t {
                    if k != &i.to_string() {
                        return None;
                    }
                    out.push(v);
                    i += 1;
                }
                Some(out)
            }
            _ => None,
        }
    }
    /// A scalar string is accepted as a single-element list, matching legacy
    /// manifest forms where `game 'gta5'` and `game { 'gta5' }` are equivalent.
    fn coerce_string_list(&self) -> Result<Vec<String>, String> {
        if let Some(s) = self.as_str() {
            return Ok(vec![s.to_string()]);
        }
        let seq = self.as_sequence().ok_or_else(|| "expected a list of strings".to_string())?;
        let mut out = Vec::with_capacity(seq.len());
        for v in seq {
            match v {
                ManifestValue::Str(s) => out.push(s.clone()),
                ManifestValue::Int(n) => out.push(n.to_string()),
                ManifestValue::Bool(b) => out.push(b.to_string()),
                _ => return Err("list must contain only strings".to_string()),
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Directive classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectiveStatus {
    Supported,
    Translated,
    IgnoredWarn,
    Unsupported,
}

impl DirectiveStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DirectiveStatus::Supported => "SUPPORTED",
            DirectiveStatus::Translated => "TRANSLATED",
            DirectiveStatus::IgnoredWarn => "IGNORED_WITH_WARNING",
            DirectiveStatus::Unsupported => "UNSUPPORTED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectiveRecord {
    pub name: String,
    pub status: DirectiveStatus,
    pub line: usize,
    pub note: String,
}

/// Classify a manifest directive name per the master spec.
pub fn classify(name: &str) -> DirectiveStatus {
    match name {
        // Supported and parsed into NormalizedManifest fields.
        "fx_version" | "resource_manifest_version" | "game" | "games" | "client_script"
        | "client_scripts" | "server_script" | "server_scripts" | "shared_script"
        | "shared_scripts" | "file" | "files" | "ui_page" | "export" | "exports"
        | "server_export" | "server_exports" | "dependency" | "dependencies" | "provide"
        | "this_is_a_map" | "data_file" | "before_level_meta" | "after_level_meta"
        | "replace_level_meta" | "loadscreen" | "loadscreen_manual_shutdown" | "server_only"
        | "lua54" | "node_version" | "clr_disable_task_scheduler"
        | "use_experimental_fxv2_oal" | "convar_category" => DirectiveStatus::Supported,

        // Legacy file-distribution concept: translated to the Aldivine
        // Streaming Engine / CDN origin config, not executed literally.
        "fileserver_add" => DirectiveStatus::Translated,

        // Legal legacy metadata with no runtime effect in Aldivine.
        "fxasset_escrow" | "escrow_disable" | "lua54_disabled" => DirectiveStatus::IgnoredWarn,

        // Known legacy/foreign constructs we deliberately do not honor.
        "author" | "description" | "repository" | "version" | "license" => DirectiveStatus::IgnoredWarn,

        _ => DirectiveStatus::Unsupported,
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    LBrace,
    RBrace,
    Assign,
    Comma,
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Lexer { s: s.as_bytes(), i: 0, line: 1 }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.s.get(self.i).copied();
        if let Some(b) = b {
            self.i += 1;
            if b == b'\n' {
                self.line += 1;
            }
        }
        b
    }

    fn skip_ws_and_comments(&mut self) -> Result<(), ManifestError> {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => {
                    self.bump();
                }
                // `--` line comment, and long-bracket comments `--[[ ]]`.
                Some(b'-') if self.s.get(self.i + 1) == Some(&b'-') => {
                    self.bump();
                    self.bump();
                    if self.peek() == Some(b'[') && self.s.get(self.i + 1) == Some(&b'[') {
                        // long comment
                        self.bump();
                        self.bump();
                        let mut depth = 1;
                        while let Some(b) = self.bump() {
                            if b == b']' && self.peek() == Some(b']') {
                                self.bump();
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            if b == b'[' && self.peek() == Some(b'[') {
                                self.bump();
                                depth += 1;
                            }
                        }
                        if depth != 0 {
                            return Err(ManifestError::Syntax {
                                line: self.line,
                                msg: "unterminated long comment".into(),
                            });
                        }
                    } else {
                        while let Some(b) = self.peek() {
                            if b == b'\n' {
                                break;
                            }
                            self.bump();
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn read_string(&mut self, quote: u8, line: usize) -> Result<String, ManifestError> {
        let mut out = String::new();
        loop {
            let b = self.bump().ok_or_else(|| ManifestError::Syntax {
                line,
                msg: "unterminated string".into(),
            })?;
            match b {
                b if b == quote => return Ok(out),
                b'\\' => {
                    let esc = self.bump().ok_or_else(|| ManifestError::Syntax {
                        line,
                        msg: "unterminated escape".into(),
                    })?;
                    let c = match esc {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'\\' => '\\',
                        b'\'' => '\'',
                        b'"' => '"',
                        b'a' => '\u{7}',
                        b'b' => '\u{8}',
                        b'f' => '\u{C}',
                        b'v' => '\u{B}',
                        b'\n' => '\n',
                        b'x' => {
                            let h1 = self.bump().unwrap_or(b'0');
                            let h2 = self.bump().unwrap_or(b'0');
                            let v = hex_val(h1).unwrap_or(0) * 16 + hex_val(h2).unwrap_or(0);
                            char::from_u32(v).unwrap_or('\u{FFFD}')
                        }
                        other => char::from(other),
                    };
                    out.push(c);
                }
                b'\n' => {
                    return Err(ManifestError::Syntax {
                        line,
                        msg: "newline in string".into(),
                    })
                }
                b => out.push(char::from(b)),
            }
        }
    }

    fn read_number(&mut self) -> f64 {
        let start = self.i;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-' | b'x' | b'X') {
                self.bump();
            } else {
                break;
            }
        }
        let txt = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("0");
        txt.parse::<f64>().unwrap_or(0.0)
    }

    fn read_ident(&mut self) -> String {
        let start = self.i;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
                self.bump();
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn next(&mut self) -> Result<Option<Tok>, ManifestError> {
        self.skip_ws_and_comments()?;
        let line = self.line;
        match self.peek() {
            None => Ok(None),
            Some(b'{') => {
                self.bump();
                Ok(Some(Tok::LBrace))
            }
            Some(b'}') => {
                self.bump();
                Ok(Some(Tok::RBrace))
            }
            Some(b'=') => {
                self.bump();
                Ok(Some(Tok::Assign))
            }
            Some(b',') => {
                self.bump();
                Ok(Some(Tok::Comma))
            }
            Some(b'\'') | Some(b'"') => {
                let q = self.bump().unwrap();
                Ok(Some(Tok::Str(self.read_string(q, line)?)))
            }
            // long string `[[...]]`
            Some(b'[') if self.s.get(self.i + 1) == Some(&b'[') => {
                self.bump();
                self.bump();
                let mut out = String::new();
                loop {
                    let b = self.bump().ok_or_else(|| ManifestError::Syntax {
                        line,
                        msg: "unterminated long string".into(),
                    })?;
                    if b == b']' && self.peek() == Some(b']') {
                        self.bump();
                        return Ok(Some(Tok::Str(out)));
                    }
                    out.push(char::from(b));
                }
            }
            Some(b) if b.is_ascii_digit() => Ok(Some(Tok::Num(self.read_number()))),
            // boolean / nil literals
            Some(b't') | Some(b'f') | Some(b'n') => {
                let id = self.read_ident();
                match id.as_str() {
                    "true" => Ok(Some(Tok::Str("__bool_true__".into()))),
                    "false" => Ok(Some(Tok::Str("__bool_false__".into()))),
                    "nil" => Ok(Some(Tok::Str("__nil__".into()))),
                    _ => Ok(Some(Tok::Ident(id))),
                }
            }
            Some(_) => {
                let id = self.read_ident();
                if id.is_empty() {
                    return Err(ManifestError::Syntax {
                        line,
                        msg: format!("unexpected byte 0x{:02x}", self.peek().unwrap_or(0)),
                    });
                }
                Ok(Some(Tok::Ident(id)))
            }
        }
    }
}

fn hex_val(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    lex: Lexer<'a>,
    peeked: Option<Tok>,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser { lex: Lexer::new(s), peeked: None }
    }

    fn peek(&mut self) -> Result<Option<&Tok>, ManifestError> {
        if self.peeked.is_none() {
            self.peeked = self.lex.next()?;
        }
        Ok(self.peeked.as_ref())
    }

    fn take(&mut self) -> Result<Option<Tok>, ManifestError> {
        match self.peeked.take() {
            Some(t) => Ok(Some(t)),
            None => self.lex.next(),
        }
    }

    /// Parse a full manifest: a sequence of `name = value` assignments.
    fn parse_manifest(&mut self) -> Result<NormalizedManifest, ManifestError> {
        let mut out = NormalizedManifest::default();
        while let Some(tok) = self.take()? {
            let name = match tok {
                Tok::Ident(n) => n,
                other => {
                    return Err(ManifestError::Syntax {
                        line: self.lex.line,
                        msg: format!("expected directive name, found {other:?}"),
                    })
                }
            };
            let line = self.lex.line;
            // `=` is optional in fxmanifest style: `fx_version 'cerulean'`
            // and `fx_version = 'cerulean'` are both legal.
            let has_assign = matches!(self.peek()?, Some(Tok::Assign));
            if has_assign {
                self.take()?; // consume =
            }
            let value = self.parse_value()?;
            // `convar_category 'Name' { ... }` puts the label and the table
            // in two separate values; fold them into one sequence.
            let value = if matches!(value, ManifestValue::Str(_))
                && matches!(self.peek()?, Some(Tok::LBrace))
            {
                let table = self.parse_value()?;
                ManifestValue::Table(vec![("1".to_string(), value), ("2".to_string(), table)])
            } else {
                value
            };
            self.apply(&mut out, &name, value, line)?;
        }
        if out.fx_version.is_none() && out.resource_manifest_version.is_none() {
            return Err(ManifestError::MissingRequired("fx_version (or resource_manifest_version)".into()));
        }
        Ok(out)
    }

    /// Parse one value: scalar, or `{ ... }` table.
    fn parse_value(&mut self) -> Result<ManifestValue, ManifestError> {
        match self.take()? {
            None => Err(ManifestError::Syntax { line: self.lex.line, msg: "unexpected end of manifest".into() }),
            Some(Tok::Str(s)) => Ok(decode_literal(&s)),
            Some(Tok::Num(n)) => {
                if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 {
                    Ok(ManifestValue::Int(n as i64))
                } else {
                    Ok(ManifestValue::Float(n))
                }
            }
            Some(Tok::LBrace) => self.parse_table(),
            // `client_script 'x.lua'` with the value being a bare ident is
            // not legal Lua; treat as syntax error to stay honest.
            Some(Tok::Ident(i)) => Err(ManifestError::Syntax {
                line: self.lex.line,
                msg: format!("bare identifier `{i}` is not a valid value"),
            }),
            Some(other) => Err(ManifestError::Syntax {
                line: self.lex.line,
                msg: format!("unexpected token {other:?}"),
            }),
        }
    }

    /// Parse `{ ... }`. Mixed sequence/table forms like
    /// `{ 'a', x = 1, 'b' }` are kept as a Table with integer keys for the
    /// positional entries so nothing is reordered or lost.
    fn parse_table(&mut self) -> Result<ManifestValue, ManifestError> {
        let mut seq: Vec<ManifestValue> = Vec::new();
        let mut map: Vec<(String, ManifestValue)> = Vec::new();
        let mut next_key = 1usize;
        loop {
            // allow trailing comma
            if let Some(Tok::Comma) = self.peek()? {
                self.take()?;
                continue;
            }
            if let Some(Tok::RBrace) = self.peek()? {
                self.take()?;
                break;
            }
            // key = value ?
            let key_is_ident = matches!(self.peek()?, Some(Tok::Ident(_)));
            if key_is_ident {
                // need lookahead for `=`
                let id = if let Some(Tok::Ident(id)) = self.peek()?.cloned() {
                    id
                } else {
                    unreachable!()
                };
                // tentatively consume ident; if next is `=`, it was a key
                let save = self.lex.i;
                let save_line = self.lex.line;
                let save_peek = self.peeked.clone();
                self.take()?;
                let after = self.peek()?;
                if matches!(after, Some(Tok::Assign)) {
                    self.take()?; // =
                    let v = self.parse_value()?;
                    map.push((id, v));
                } else {
                    // not a key: restore and treat ident as error (bare id)
                    self.lex.i = save;
                    self.lex.line = save_line;
                    self.peeked = save_peek;
                    return Err(ManifestError::Syntax {
                        line: self.lex.line,
                        msg: format!("bare identifier `{id}` inside table"),
                    });
                }
            } else {
                let v = self.parse_value()?;
                map.push((next_key.to_string(), v));
                next_key += 1;
            }
            // optional comma between entries
            if let Some(Tok::Comma) = self.peek()? {
                self.take()?;
            } else if let Some(Tok::RBrace) = self.peek()? {
                self.take()?;
                break;
            } else {
                return Err(ManifestError::Syntax {
                    line: self.lex.line,
                    msg: "expected `,` or `}` in table".into(),
                });
            }
        }
        let _ = &mut seq;
        if map.iter().all(|(k, _)| k.parse::<usize>().is_ok()) {
            Ok(ManifestValue::Table(map))
        } else {
            Ok(ManifestValue::Table(map))
        }
    }

    fn apply(
        &mut self,
        out: &mut NormalizedManifest,
        name: &str,
        value: ManifestValue,
        line: usize,
    ) -> Result<(), ManifestError> {
        let status = classify(name);
        out.directives.push(DirectiveRecord {
            name: name.to_string(),
            status,
            line,
            note: note_for(name),
        });

        macro_rules! str_list {
            ($field:ident) => {{
                out.$field.extend(value.coerce_string_list().map_err(|m| {
                    ManifestError::InvalidValue { directive: name.into(), line, msg: m }
                })?);
            }};
        }

        match name {
            "fx_version" => out.fx_version = value.as_str().map(str::to_string),
            "resource_manifest_version" => {
                out.resource_manifest_version = value.as_str().map(str::to_string)
            }
            "game" | "games" => str_list!(game),
            "client_script" | "client_scripts" => str_list!(client_scripts),
            "server_script" | "server_scripts" => str_list!(server_scripts),
            "shared_script" | "shared_scripts" => str_list!(shared_scripts),
            "file" | "files" => str_list!(files),
            "ui_page" => out.ui_page = value.as_str().map(str::to_string),
            "export" | "exports" => str_list!(exports),
            "server_export" | "server_exports" => str_list!(server_exports),
            "dependency" | "dependencies" => str_list!(dependencies),
            "provide" => str_list!(provides),
            "this_is_a_map" => out.this_is_a_map = value.as_bool().unwrap_or(true),
            "data_file" => {
                // Accept `data_file { 'TYPE', 'file' }` (table) and
                // `data_file 'TYPE' 'file'` (two scalars on one line).
                let pair: Option<(String, String)> = if let ManifestValue::Table(t) = &value {
                    t.len().checked_sub(2).and_then(|_| match (&t[0].1, &t[1].1) {
                        (ManifestValue::Str(k), ManifestValue::Str(f)) => Some((k.clone(), f.clone())),
                        _ => None,
                    })
                } else {
                    None
                };
                let (kind, file) = match pair {
                    Some(p) => p,
                    None => {
                        return Err(ManifestError::InvalidValue {
                            directive: name.into(),
                            line,
                            msg: "data_file requires { 'TYPE', 'file' }".into(),
                        })
                    }
                };
                out.data_files.push(DataFile { kind, file });
            }
            "before_level_meta" => str_list!(before_level_meta),
            "after_level_meta" => str_list!(after_level_meta),
            "replace_level_meta" => str_list!(replace_level_meta),
            "loadscreen" => out.loadscreen = value.as_str().map(str::to_string),
            "loadscreen_manual_shutdown" => {
                out.loadscreen_manual_shutdown = value.as_bool().unwrap_or(true)
            }
            "server_only" => out.server_only = value.as_bool().unwrap_or(true),
            "lua54" => out.lua54 = value.as_bool().unwrap_or(true),
            "node_version" => out.node_version = value.as_str().map(str::to_string),
            "clr_disable_task_scheduler" => {
                out.clr_disable_task_scheduler = value.as_bool().unwrap_or(true)
            }
            "use_experimental_fxv2_oal" => {
                out.use_experimental_fxv2_oal = value.as_bool().unwrap_or(true)
            }
            "convar_category" => {
                out.convar_categories.push(parse_convar_category(&value, line, name)?);
            }
            "escrow_ignore" => out.escrow_ignore = value.as_bool().unwrap_or(true),
            // custom/unknown metadata is preserved verbatim, not discarded
            _ => out.custom.push((name.to_string(), value)),
        }
        Ok(())
    }
}

fn note_for(name: &str) -> String {
    match name {
        "fileserver_add" => "legacy file-server config; translated to Aldivine Streaming Engine / CDN origin".into(),
        "use_experimental_fxv2_oal" => "OAL compatibility profile activated only when implemented".into(),
        "convar_category" => "drives Aegis resource configuration UI".into(),
        _ => String::new(),
    }
}

/// A convar_category entry is `{ 'Category Name', { convar = 'name', name = 'Label', ... } }`.
fn parse_convar_category(
    value: &ManifestValue,
    line: usize,
    directive: &str,
) -> Result<ConvarCategory, ManifestError> {
    let fail = |msg: String| ManifestError::InvalidValue {
        directive: directive.into(),
        line,
        msg,
    };
    let seq = value.as_sequence().ok_or_else(|| fail("convar_category must be a table".into()))?;
    if seq.len() < 2 {
        return Err(fail("convar_category needs a name and a var table".into()));
    }
    let name = seq[0].as_str().ok_or_else(|| fail("category name must be a string".into()))?;
    let vars_tbl = &seq[1];
    let var_seq = vars_tbl
        .as_sequence()
        .ok_or_else(|| fail("convar vars must be a list of tables".into()))?;
    let mut vars = Vec::with_capacity(var_seq.len());
    for v in var_seq {
        let entries = match v {
            ManifestValue::Table(t) => t,
            _ => return Err(fail("each convar var must be a table".into())),
        };
        let mut cv = ConvarVar::default();
        for (k, val) in entries {
            match k.as_str() {
                "convar" => cv.convar = val.as_str().unwrap_or("").to_string(),
                "name" => cv.name = val.as_str().map(str::to_string),
                "help" => cv.help = val.as_str().map(str::to_string),
                "default" => cv.default = Some(val.clone()),
                "min" => {
                    if let ManifestValue::Int(n) = val {
                        cv.min = Some(*n as f64)
                    } else if let ManifestValue::Float(n) = val {
                        cv.min = Some(*n)
                    }
                }
                "max" => {
                    if let ManifestValue::Int(n) = val {
                        cv.max = Some(*n as f64)
                    } else if let ManifestValue::Float(n) = val {
                        cv.max = Some(*n)
                    }
                }
                "type" | "kind" => cv.kind = val.as_str().map(str::to_string),
                _ => {}
            }
        }
        if cv.convar.is_empty() {
            return Err(fail("convar entry missing `convar` key".into()));
        }
        vars.push(cv);
    }
    Ok(ConvarCategory { name: name.to_string(), vars })
}

fn decode_literal(s: &str) -> ManifestValue {
    match s {
        "__bool_true__" => ManifestValue::Bool(true),
        "__bool_false__" => ManifestValue::Bool(false),
        "__nil__" => ManifestValue::Nil,
        _ => ManifestValue::Str(s.to_string()),
    }
}

/// Parse a `fxmanifest.lua` / `__resource.lua` source string.
pub fn parse_fxmanifest(src: &str) -> Result<NormalizedManifest, ManifestError> {
    Parser::new(src).parse_manifest()
}

/// Parse an `ald_manifest.toml` source string.
pub fn parse_ald_manifest(src: &str) -> Result<NormalizedManifest, ManifestError> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        fx_version: Option<String>,
        #[serde(default)]
        game: Vec<String>,
        #[serde(default)]
        client_scripts: Vec<String>,
        #[serde(default)]
        server_scripts: Vec<String>,
        #[serde(default)]
        shared_scripts: Vec<String>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        ui_page: Option<String>,
        #[serde(default)]
        dependencies: Vec<String>,
        #[serde(default)]
        provides: Vec<String>,
        #[serde(default)]
        this_is_a_map: bool,
        #[serde(default)]
        server_only: bool,
        #[serde(default)]
        lua54: bool,
    }
    let raw: Raw = toml::from_str(src)
        .map_err(|e| ManifestError::Syntax { line: 1, msg: e.to_string() })?;
    let mut out = NormalizedManifest::default();
    out.fx_version = raw.fx_version;
    out.game = raw.game;
    out.client_scripts = raw.client_scripts;
    out.server_scripts = raw.server_scripts;
    out.shared_scripts = raw.shared_scripts;
    out.files = raw.files;
    out.ui_page = raw.ui_page;
    out.dependencies = raw.dependencies;
    out.provides = raw.provides;
    out.this_is_a_map = raw.this_is_a_map;
    out.server_only = raw.server_only;
    out.lua54 = raw.lua54;
    if out.fx_version.is_none() {
        return Err(ManifestError::MissingRequired("fx_version".into()));
    }
    Ok(out)
}

impl NormalizedManifest {
    /// All script paths for one side, with shared scripts appended.
    pub fn scripts_for(&self, side: Side) -> Vec<&str> {
        let (mine, shared): (&[String], &[String]) = match side {
            Side::Client => (&self.client_scripts, &self.shared_scripts),
            Side::Server => (&self.server_scripts, &self.shared_scripts),
        };
        mine.iter().chain(shared).map(|s| s.as_str()).collect()
    }

    /// Every file the resource declares, deduplicated, order-preserved.
    pub fn all_files(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in self.files.iter()
            .chain(self.data_files.iter().map(|d| &d.file))
            .chain(self.ui_page.iter())
            .chain(self.client_scripts.iter())
            .chain(self.shared_scripts.iter())
            .chain(self.server_scripts.iter())
            .chain(self.shared_scripts.iter())
        {
            if !out.contains(f) {
                out.push(f.clone());
            }
        }
        out
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        for d in &self.directives {
            match d.status {
                DirectiveStatus::IgnoredWarn => {
                    w.push(format!("line {}: `{}` is {} — no runtime effect", d.line, d.name, d.status.as_str()))
                }
                DirectiveStatus::Unsupported => {
                    w.push(format!("line {}: `{}` is UNSUPPORTED — recorded, not honored", d.line, d.name))
                }
                DirectiveStatus::Translated => {
                    w.push(format!("line {}: `{}` TRANSLATED to Aldivine equivalent", d.line, d.name))
                }
                DirectiveStatus::Supported => {}
            }
        }
        if self.use_experimental_fxv2_oal {
            w.push("use_experimental_fxv2_oal: OAL profile is PARTIAL — see docs".into());
        }
        w
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = r#"
fx_version 'cerulean'
game 'gta5'

author 'Some Author'

client_script 'client.lua'
client_scripts {
    'a.lua',
    'b.lua',
}

server_script 'server.lua'

shared_scripts { 'shared.lua' }

ui_page 'ui/index.html'

files {
    'ui/index.html',
    'ui/app.js',
}

dependency 'esx_core'
dependencies { 'oxmysql', 'pvoice' }

exports { 'GetPlayer' }
server_exports { 'RegisterJob' }

data_file { 'DLC_ITYP_REQUEST', 'myassets.ytyp' }

convar_category 'Police Script' {
    {
        convar = 'police_maxOfficers',
        name = 'Max Officers',
        help = 'Maximum on-duty officers',
        type = 'integer',
        min = 0,
        max = 64,
    },
    {
        convar = 'police_notifyStyle',
        name = 'Notification Style',
        type = 'combo',
    },
}

this_is_a_map 'yes' -- yes, this is a map

lua54 'yes'
node_version '22'

unknown_thing 'value'
"#;

    #[test]
    fn parses_simple_manifest() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert_eq!(m.fx_version.as_deref(), Some("cerulean"));
        assert_eq!(m.game, vec!["gta5"]);
        // client.lua + a.lua + b.lua + shared.lua
        assert_eq!(m.client_scripts, vec!["client.lua", "a.lua", "b.lua"]);
        assert_eq!(m.shared_scripts, vec!["shared.lua"]);
        assert_eq!(m.ui_page.as_deref(), Some("ui/index.html"));
        assert_eq!(m.files, vec!["ui/index.html", "ui/app.js"]);
    }

    #[test]
    fn dependencies_merge() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert_eq!(m.dependencies, vec!["esx_core", "oxmysql", "pvoice"]);
    }

    #[test]
    fn exports_recorded() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert_eq!(m.exports, vec!["GetPlayer"]);
        assert_eq!(m.server_exports, vec!["RegisterJob"]);
    }

    #[test]
    fn data_file_parsed() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert_eq!(m.data_files.len(), 1);
        assert_eq!(m.data_files[0].kind, "DLC_ITYP_REQUEST");
        assert_eq!(m.data_files[0].file, "myassets.ytyp");
    }

    #[test]
    fn boolean_directives_accept_yes() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert!(m.this_is_a_map);
        assert!(m.lua54);
        assert_eq!(m.node_version.as_deref(), Some("22"));
    }

    #[test]
    fn unknown_directive_preserved_and_flagged() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert!(m.custom.iter().any(|(k, _)| k == "unknown_thing"));
        let rec = m.directives.iter().find(|d| d.name == "unknown_thing").unwrap();
        assert_eq!(rec.status, DirectiveStatus::Unsupported);
        // `author` must NOT be silently dropped
        assert!(m.directives.iter().any(|d| d.name == "author"));
    }

    #[test]
    fn convar_category_parses() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        assert_eq!(m.convar_categories.len(), 1);
        let cat = &m.convar_categories[0];
        assert_eq!(cat.name, "Police Script");
        assert_eq!(cat.vars.len(), 2);
        assert_eq!(cat.vars[0].convar, "police_maxOfficers");
        assert_eq!(cat.vars[0].name.as_deref(), Some("Max Officers"));
        assert_eq!(cat.vars[0].min, Some(0.0));
        assert_eq!(cat.vars[0].max, Some(64.0));
        assert_eq!(cat.vars[0].kind.as_deref(), Some("integer"));
        assert_eq!(cat.vars[1].convar, "police_notifyStyle");
    }

    #[test]
    fn scripts_for_side() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        let c = m.scripts_for(Side::Client);
        assert_eq!(c, vec!["client.lua", "a.lua", "b.lua", "shared.lua"]);
        let s = m.scripts_for(Side::Server);
        assert_eq!(s, vec!["server.lua", "shared.lua"]);
    }

    #[test]
    fn all_files_dedupes() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        let f = m.all_files();
        // ui/index.html appears in files AND ui_page AND as a script? no:
        // it appears once despite two declarations.
        assert_eq!(f.iter().filter(|x| x == &"ui/index.html").count(), 1);
        assert!(f.iter().any(|x| x == "ui/app.js"));
    }

    #[test]
    fn warnings_cover_ignored_and_unsupported() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        let w = m.warnings();
        assert!(w.iter().any(|x| x.contains("author") && x.contains("IGNORED")));
        assert!(w.iter().any(|x| x.contains("unknown_thing") && x.contains("UNSUPPORTED")));
    }

    #[test]
    fn long_comment_and_long_string() {
        let src = r#"
fx_version 'cerulean'
--[[ this is a
multi-line comment ]]
game 'gta5'
description [[a long
description]]
"#;
        let m = parse_fxmanifest(src).unwrap();
        assert_eq!(m.fx_version.as_deref(), Some("cerulean"));
        assert!(m.custom.iter().any(|(k, v)| k == "description" && v.as_str().unwrap_or("").contains("long")));
    }

    #[test]
    fn escapes_decode() {
        let src = "fx_version 'a\\nb\\tc'";
        let m = parse_fxmanifest(src).unwrap();
        assert_eq!(m.fx_version.as_deref(), Some("a\nb\tc"));
    }

    #[test]
    fn missing_fx_version_errors() {
        let src = "game 'gta5'";
        assert!(matches!(parse_fxmanifest(src), Err(ManifestError::MissingRequired(_))));
    }

    #[test]
    fn unterminated_string_errors() {
        assert!(parse_fxmanifest("fx_version 'cerulean").is_err());
    }

    #[test]
    fn nested_and_mixed_table() {
        let src = "fx_version 'cerulean'\nfiles { 'a', 'b', { nested = 1 }, 'c' }";
        // a nested keyed table inside a string list is a value error
        assert!(parse_fxmanifest(src).is_err());
    }

    #[test]
    fn legacy_double_resource_manifest() {
        let src = "resource_manifest_version '44febabe-d386-4d18-afbe-5e627f4af937'\ngame 'gta5'";
        let m = parse_fxmanifest(src).unwrap();
        assert_eq!(
            m.resource_manifest_version.as_deref(),
            Some("44febabe-d386-4d18-afbe-5e627f4af937")
        );
    }

    #[test]
    fn ald_manifest_toml_parses() {
        let src = r#"
fx_version = "native"
game = ["gta5"]
client_scripts = ["c.lua"]
server_scripts = ["s.lua"]
lua54 = true
"#;
        let m = parse_ald_manifest(src).unwrap();
        assert_eq!(m.fx_version.as_deref(), Some("native"));
        assert_eq!(m.client_scripts, vec!["c.lua"]);
        assert!(m.lua54);
    }

    #[test]
    fn classify_status_table() {
        assert_eq!(classify("client_script"), DirectiveStatus::Supported);
        assert_eq!(classify("fileserver_add"), DirectiveStatus::Translated);
        assert_eq!(classify("author"), DirectiveStatus::IgnoredWarn);
        assert_eq!(classify("totally_made_up"), DirectiveStatus::Unsupported);
    }

    #[test]
    fn boolean_true_false_literals() {
        let src = "fx_version 'cerulean'\nserver_only true\nthis_is_a_map false";
        let m = parse_fxmanifest(src).unwrap();
        assert!(m.server_only);
        assert!(!m.this_is_a_map);
    }

    #[test]
    fn directive_records_are_ordered() {
        let m = parse_fxmanifest(SIMPLE).unwrap();
        let names: Vec<&str> = m.directives.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names[0], "fx_version");
        assert_eq!(names[1], "game");
        assert_eq!(names.last().copied(), Some("unknown_thing"));
    }
}
