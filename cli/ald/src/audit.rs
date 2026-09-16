//! `ald dependencies audit-native` — CI gate for native-dependency isolation.
//!
//! The native Aldivine path (audited roots, currently `ald-server`) must not
//! pull native-compatibility runtimes or C toolchains into its dependency
//! closure. This command runs `cargo tree --offline` per root and fails on
//! any denylisted crate. Exit codes: 0 = PASS, 1 = violations found,
//! 2 = usage or environment error (cargo missing, not a workspace, ...).

use std::process::{exit, Command};

/// Native-profile roots: the shipped native path. Everything here must be
/// pure Rust plus OS API shims.
pub const AUDITED_ROOTS: &[&str] = &["ald-server"];

/// Native-boundary crates that must never appear in a native root closure.
/// Exact crate-name matches (first token of `cargo tree --prefix none` lines).
pub const DENY: &[&str] = &[
    "libloading",
    "mlua",
    "rquickjs",
    "boa_engine",
    "boa_runtime",
    "v8",
    "deno_core",
    "napi",
    "node_api",
    "cef",
    "clang-sys",
    "bindgen",
    "openssl-sys",
    "cmake",
];

/// Parse `cargo tree --prefix none` output into crate names.
pub fn dep_names(tree_output: &str) -> Vec<String> {
    tree_output
        .lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            if token.is_empty() || token.starts_with('(') {
                return None;
            }
            Some(token.to_string())
        })
        .collect()
}

/// Denylisted crates present in `names`, in DENY order, deduplicated.
pub fn violations(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for denied in DENY {
        if names.iter().any(|n| n == denied) && !out.iter().any(|o| o == denied) {
            out.push(denied.to_string());
        }
    }
    out
}

pub fn run(args: &[String]) {
    if args.first().map(String::as_str) != Some("audit-native") {
        eprintln!("usage: ald dependencies audit-native");
        exit(2);
    }
    if !std::path::Path::new("Cargo.toml").exists() || !std::path::Path::new("Cargo.lock").exists() {
        eprintln!("audit-native must run from the workspace root (Cargo.toml + Cargo.lock not found here)");
        exit(2);
    }
    let mut failed = false;
    for root in AUDITED_ROOTS {
        let output =
            Command::new("cargo").args(["tree", "-p", root, "-e", "no-dev", "--offline", "--prefix", "none"]).output();
        let output = match output {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                eprintln!(
                    "{root}: cargo tree failed: {}",
                    String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("")
                );
                exit(2);
            }
            Err(e) => {
                eprintln!("{root}: cannot run cargo: {e}");
                exit(2);
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let names = dep_names(&stdout);
        let bad = violations(&names);
        if bad.is_empty() {
            println!("{root}: PASS ({} crates, no native-boundary deps)", names.len());
        } else {
            println!("{root}: FAIL unexpected native deps: {}", bad.join(", "));
            failed = true;
        }
    }
    if failed {
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_prefix_none() {
        let sample = "ald-server v0.1.0\ntokio v1.0.0\nwindows-sys v0.61.2\n";
        assert_eq!(dep_names(sample), vec!["ald-server", "tokio", "windows-sys"]);
    }

    #[test]
    fn violations_match_exact_names() {
        let names = vec!["tokio".into(), "mlua".into(), "rquickjs".into(), "serde".into()];
        assert_eq!(violations(&names), vec!["mlua".to_string(), "rquickjs".to_string()]);
        // Substring lookalikes do not match: exact crate names only.
        let names = vec!["mlua-something".into(), "my-rquickjs".into()];
        assert!(violations(&names).is_empty());
    }

    #[test]
    fn empty_tree_is_clean() {
        assert!(violations(&[]).is_empty());
    }
}
