//! `exec config/x.cfg` include resolution with traversal, depth and cycle guards.
//!
//! Security model:
//! - included paths are always resolved relative to the config root, never CWD
//! - `..` is rejected outright (operators organize configs under the root)
//! - absolute paths are rejected (they can escape the root)
//! - the include chain is tracked so a cycle fails fast instead of recursing to the stack limit
//! - depth is capped so a malicious/degenerate include tree cannot blow the stack

use crate::ast::{Document, LineKind};
use crate::error::{CfgError, CfgResult};
use crate::parse::{parse_document, ParseOptions};

/// Maximum nested `exec` depth. Generous for real layouts; shallow enough to be safe.
pub const MAX_INCLUDE_DEPTH: usize = 16;

/// Virtual filesystem for config reads. Real deployment implements this over the
/// server-data directory; tests use [`InMemoryFs`].
pub trait Vfs {
    fn read(&self, path: &str) -> Option<String>;
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryFs {
    files: std::collections::BTreeMap<String, String>,
}

impl InMemoryFs {
    pub fn new() -> Self {
        InMemoryFs { files: Default::default() }
    }

    pub fn insert(&mut self, path: &str, contents: &str) -> &mut Self {
        self.files.insert(Self::norm(path.to_string()), contents.to_string());
        self
    }
}

impl Vfs for InMemoryFs {
    fn read(&self, path: &str) -> Option<String> {
        self.files.get(&Self::norm(path.to_string())).cloned()
    }
}

impl InMemoryFs {
    fn norm(p: String) -> String {
        p.trim_start_matches("./").replace(std::path::MAIN_SEPARATOR, "/")
    }
}

pub struct IncludeResolver<'fs> {
    fs: &'fs dyn Vfs,
    opts: ParseOptions,
}

impl<'fs> IncludeResolver<'fs> {
    pub fn new(fs: &'fs dyn Vfs) -> Self {
        IncludeResolver { fs, opts: ParseOptions::default() }
    }

    pub fn with_options(mut self, opts: ParseOptions) -> Self {
        self.opts = opts;
        self
    }

    /// Parse `root_path` and expand every `exec` in place.
    ///
    /// Returns a single flat [`Document`] whose `source` is the root path. Included
    /// directives are inlined in order; their original line numbers are remapped to
    /// their position in the flattened document so errors point at the merged file.
    pub fn resolve(&self, root_path: &str) -> CfgResult<Document> {
        let text =
            self.fs.read(root_path).ok_or_else(|| CfgError::IncludeMissing { line: 0, path: root_path.to_string() })?;
        let root = parse_document(root_path, &text, &self.opts)?;
        let mut out = Document::new(root_path);
        let mut visited: Vec<String> = vec![normalize_owned(root_path)];
        self.expand(root, &mut out, &mut visited, 1, 0)?;
        Ok(out)
    }

    fn expand(
        &self,
        doc: Document,
        out: &mut Document,
        chain: &mut Vec<String>,
        depth: usize,
        parent_line: u32,
    ) -> CfgResult<()> {
        for (i, line) in doc.lines.into_iter().enumerate() {
            let line_no = (i as u32) + 1;
            match line.kind {
                LineKind::Directive(ref d) if d.verb == "exec" => {
                    let target = d.first_arg().ok_or(CfgError::WrongArgCount {
                        line: parent_line.max(line_no),
                        verb: "exec".into(),
                        expected: 1,
                        got: 0,
                    })?;
                    self.expand_include(target, out, chain, depth, line_no)?;
                }
                _ => out.push(line),
            }
        }
        Ok(())
    }

    fn expand_include(
        &self,
        target: &str,
        out: &mut Document,
        chain: &mut Vec<String>,
        depth: usize,
        at_line: u32,
    ) -> CfgResult<()> {
        if target.is_empty() {
            return Err(CfgError::InvalidValue { line: at_line, reason: "empty exec path".into() });
        }
        // Absolute paths and traversal are refused: includes must stay under the config root.
        // Note: on Windows, `Path::is_absolute()` is false for "/etc/passwd", so leading
        // separators are rejected explicitly — a POSIX absolute path must not slip through.
        if target.starts_with('/') || target.starts_with('\\') || std::path::Path::new(target).is_absolute() {
            return Err(CfgError::IncludeOutsideRoot { line: at_line, path: target.into() });
        }
        if target.split(['/', '\\']).any(|c| c == "..") {
            return Err(CfgError::IncludeTraversal { line: at_line, path: target.into() });
        }
        if depth >= MAX_INCLUDE_DEPTH {
            return Err(CfgError::IncludeDepthLimit { line: at_line, path: target.into(), limit: MAX_INCLUDE_DEPTH });
        }
        let norm = normalize_owned(target);
        if chain.contains(&norm) {
            let chain_str = chain.join(" -> ");
            return Err(CfgError::IncludeCycle { line: at_line, path: target.into(), chain: chain_str });
        }

        let text = self.fs.read(&norm).ok_or_else(|| CfgError::IncludeMissing { line: at_line, path: norm.clone() })?;
        let child = parse_document(&norm, &text, &self.opts)?;
        chain.push(norm);
        self.expand(child, out, chain, depth + 1, at_line)?;
        chain.pop();
        Ok(())
    }
}

fn normalize_owned(p: &str) -> String {
    p.trim_start_matches("./").replace('\\', "/").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Directive;

    use crate::error::is_root_only;

    fn resolver(files: &[(&str, &str)]) -> (InMemoryFs, IncludeResolver<'static>) {
        // The leak is test-only and keeps the borrow simple.
        let fs: &'static mut InMemoryFs = Box::leak(Box::new(InMemoryFs::new()));
        for (p, c) in files {
            fs.insert(p, c);
        }
        (fs.clone(), IncludeResolver::new(fs))
    }

    fn first_dir(doc: &Document) -> &Directive {
        doc.directives().next().expect("at least one directive").1
    }

    #[test]
    fn inlines_nested_include() {
        let (_, r) = resolver(&[
            ("server.cfg", "set a 1\nexec config/db.cfg\nset b 2\n"),
            ("config/db.cfg", "set db_url \"postgres://x\"\n"),
        ]);
        let doc = r.resolve("server.cfg").unwrap();
        let dirs: Vec<String> = doc.directives().map(|(_, d)| d.verb.clone()).collect();
        assert_eq!(dirs, vec!["set", "set", "set"]);
        let vals: Vec<String> = doc.directives().map(|(_, d)| d.kv().unwrap().1.to_string()).collect();
        assert_eq!(vals, vec!["1", "postgres://x", "2"]);
    }

    #[test]
    fn include_missing_fails() {
        let (_, r) = resolver(&[("server.cfg", "exec config/nope.cfg\n")]);
        let e = r.resolve("server.cfg").unwrap_err();
        assert!(matches!(e, CfgError::IncludeMissing { .. }));
    }

    #[test]
    fn traversal_rejected() {
        let (_, r) = resolver(&[("server.cfg", "exec ../secret.cfg\n"), ("secret.cfg", "set_secret k v\n")]);
        let e = r.resolve("server.cfg").unwrap_err();
        assert!(matches!(e, CfgError::IncludeTraversal { .. }));
    }

    #[test]
    fn absolute_include_rejected() {
        let (_, r) = resolver(&[("server.cfg", "exec /etc/passwd\n")]);
        let e = r.resolve("server.cfg").unwrap_err();
        assert!(matches!(e, CfgError::IncludeOutsideRoot { .. }));
    }

    #[test]
    fn direct_cycle_rejected() {
        let (_, r) = resolver(&[("a.cfg", "exec b.cfg\n"), ("b.cfg", "exec a.cfg\n")]);
        let e = r.resolve("a.cfg").unwrap_err();
        assert!(matches!(e, CfgError::IncludeCycle { .. }));
    }

    #[test]
    fn self_cycle_rejected() {
        let (_, r) = resolver(&[("a.cfg", "exec a.cfg\n")]);
        assert!(matches!(r.resolve("a.cfg").unwrap_err(), CfgError::IncludeCycle { .. }));
    }

    #[test]
    fn deep_chain_within_limit_ok() {
        // 8-deep chain is under MAX_INCLUDE_DEPTH (16).
        let fs: &'static mut InMemoryFs = Box::leak(Box::new(InMemoryFs::new()));
        for i in 0..8 {
            let cur = if i == 0 { "root.cfg" } else { &format!("f{i}.cfg")[..] };
            fs.insert(cur, &format!("exec f{}.cfg\nset v {i}\n", i + 1));
        }
        // Terminal node: terminates the chain without a further include.
        fs.insert("f8.cfg", "set v 8\n");
        let r = IncludeResolver::new(fs);
        let doc = r.resolve("root.cfg").unwrap();
        assert_eq!(doc.directives().count(), 9);
    }

    #[test]
    fn include_depth_limit_enforced() {
        // 20-deep chain exceeds MAX_INCLUDE_DEPTH (16). Nodes f1..f19 all chain deeper;
        // the missing terminal file is irrelevant: the depth guard fires first.
        let fs: &'static mut InMemoryFs = Box::leak(Box::new(InMemoryFs::new()));
        for i in 0..20 {
            let cur = if i == 0 { "root.cfg" } else { &format!("f{i}.cfg")[..] };
            fs.insert(cur, &format!("exec f{}.cfg\nset v {i}\n", i + 1));
        }
        let r = IncludeResolver::new(fs);
        let e = r.resolve("root.cfg").unwrap_err();
        assert!(matches!(e, CfgError::IncludeDepthLimit { limit: MAX_INCLUDE_DEPTH, .. }));
    }

    #[test]
    fn root_missing_fails() {
        let (_, r) = resolver(&[]);
        assert!(matches!(r.resolve("server.cfg").unwrap_err(), CfgError::IncludeMissing { .. }));
    }

    #[test]
    fn backslash_paths_normalized() {
        let (_, r) = resolver(&[("server.cfg", "exec config\\db.cfg\n"), ("config/db.cfg", "set db 1\n")]);
        let doc = r.resolve("server.cfg").unwrap();
        assert_eq!(first_dir(&doc).kv().unwrap().1, "1");
    }

    #[test]
    fn include_of_quoted_path() {
        let (_, r) = resolver(&[("server.cfg", "exec \"config/db.cfg\"\n"), ("config/db.cfg", "set db 1\n")]);
        let doc = r.resolve("server.cfg").unwrap();
        assert_eq!(first_dir(&doc).kv().unwrap().1, "1");
    }

    #[test]
    fn nested_three_levels() {
        let (_, r) =
            resolver(&[("server.cfg", "exec a.cfg\n"), ("a.cfg", "exec b.cfg\n"), ("b.cfg", "set deep true\n")]);
        let doc = r.resolve("server.cfg").unwrap();
        assert_eq!(first_dir(&doc).kv().unwrap(), ("deep", "true"));
    }

    #[test]
    fn comments_in_included_files_preserved() {
        let (_, r) = resolver(&[("server.cfg", "exec a.cfg\n"), ("a.cfg", "# note\nset x 1\n")]);
        let doc = r.resolve("server.cfg").unwrap();
        assert!(matches!(doc.lines[0].kind, LineKind::Comment(_)));
    }

    #[test]
    fn empty_include_directive_fails() {
        let (_, r) = resolver(&[("server.cfg", "exec\n")]);
        let e = r.resolve("server.cfg").unwrap_err();
        assert!(matches!(e, CfgError::WrongArgCount { verb, .. } if verb == "exec"));
    }

    #[test]
    fn same_file_included_twice_not_a_cycle() {
        // Two sibling includes of the same file: allowed (no recursion).
        let (_, r) = resolver(&[("server.cfg", "exec a.cfg\nexec a.cfg\n"), ("a.cfg", "set x 1\n")]);
        let doc = r.resolve("server.cfg").unwrap();
        assert_eq!(doc.directives().count(), 2);
    }

    #[test]
    fn diamond_include_not_a_cycle() {
        let (_, r) = resolver(&[
            ("server.cfg", "exec a.cfg\nexec b.cfg\n"),
            ("a.cfg", "exec shared.cfg\n"),
            ("b.cfg", "exec shared.cfg\n"),
            ("shared.cfg", "set shared 1\n"),
        ]);
        let doc = r.resolve("server.cfg").unwrap();
        assert_eq!(doc.directives().count(), 2);
    }

    #[test]
    fn is_root_only_marker() {
        assert!(is_root_only("exec"));
        assert!(!is_root_only("set"));
    }

    #[test]
    fn env_expansion_in_included_file() {
        std::env::set_var("ALD_CFG_TEST_DB", "pg://included");
        let (_, r) = resolver(&[("server.cfg", "exec a.cfg\n"), ("a.cfg", "set db \"${ALD_CFG_TEST_DB}\"\n")]);
        let doc = r.resolve("server.cfg").unwrap();
        assert_eq!(first_dir(&doc).kv().unwrap().1, "pg://included");
        std::env::remove_var("ALD_CFG_TEST_DB");
    }
}
