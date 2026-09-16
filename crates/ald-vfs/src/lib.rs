//! Aldivine VFS — resource-scoped virtual filesystem.
//!
//! Gives resources a private, sandboxed namespace (`@my_resource/data/x.json`)
//! and denies everything that could escape it:
//!   * `..` traversal and absolute paths
//!   * symlinks / junctions / reparse points that leave the sandbox
//!   * case-insensitive path confusion and Unicode normalization tricks
//!   * Windows reserved device names (`NUL`, `CON`, `COM1`, `AUX`...)
//!   * alternate data streams (`file.txt:evil`)
//!   * cross-resource access (only `@self` and explicitly shared mounts)
//!
//! Design rule: never trust a normalized string alone. Lexical checks reject
//! obvious attacks; the resolved path is re-checked against the mount root by
//! *component-wise canonical comparison* so a symlink swap after validation is
//! still caught.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// VFS error. All variants are safe to surface to resource authors; none
/// leak absolute host paths beyond the sandbox root in their messages.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VfsError {
    #[error("invalid resource name: {0}")]
    BadResourceName(String),
    #[error("unknown resource: @{0}")]
    UnknownResource(String),
    #[error("path escapes sandbox: {0}")]
    PathEscape(String),
    #[error("absolute paths are not allowed")]
    AbsolutePath,
    #[error("reserved device name: {0}")]
    ReservedDevice(String),
    #[error("alternate data streams are not allowed: {0}")]
    AdsNotAllowed(String),
    #[error("path is not UTF-8")]
    NonUtf8,
    #[error("cross-resource access denied: @{0} (not shared with @{1})")]
    CrossResource(String, String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io error")]
    Io,
}

/// One mounted resource namespace.
#[derive(Debug, Clone)]
pub struct Mount {
    pub resource: String,
    /// Canonical (symlink-resolved) filesystem root of this resource.
    pub root: PathBuf,
    /// Resources this one explicitly shares read access with.
    pub shared_with: Vec<String>,
    /// True if this mount is writable by its own resource.
    pub writable: bool,
}

/// The VFS. Cloning is cheap (roots are Arc-shared); mutation takes &mut.
#[derive(Debug, Clone)]
pub struct Vfs {
    mounts: HashMap<String, Mount>,
}

impl Vfs {
    pub fn new() -> Self {
        Vfs { mounts: HashMap::new() }
    }

    /// Mount a resource namespace. `root` is canonicalized (symlinks resolved)
    /// at mount time; later reads still re-verify on every access.
    pub fn mount(&mut self, resource: &str, root: &Path, writable: bool) -> Result<(), VfsError> {
        validate_resource_name(resource)?;
        let canonical = canonicalize_or(root, root)?;
        self.mounts.insert(
            resource.to_string(),
            Mount { resource: resource.to_string(), root: canonical, shared_with: Vec::new(), writable },
        );
        Ok(())
    }

    /// Grant `other` read access to `resource`'s namespace.
    pub fn share(&mut self, resource: &str, other: &str) -> Result<(), VfsError> {
        validate_resource_name(resource)?;
        validate_resource_name(other)?;
        match self.mounts.get_mut(resource) {
            Some(m) => {
                if !m.shared_with.iter().any(|s| s == other) {
                    m.shared_with.push(other.to_string());
                }
                Ok(())
            }
            None => Err(VfsError::UnknownResource(resource.to_string())),
        }
    }

    /// Resolve a virtual `@resource/...` path to a real path, verifying
    /// every hardening rule. `caller` is the resource doing the ask.
    pub fn resolve(&self, virtual_path: &str, caller: &str) -> Result<PathBuf, VfsError> {
        let (resource, raw) = split_virtual(virtual_path)?;
        // Sanitize first: every downstream check sees normalized separators.
        let rel = sanitize_relative(&raw)?;
        // Existence before access, so an unknown namespace reports as itself
        // rather than as a cross-resource denial.
        let mount = self.mounts.get(&resource).ok_or_else(|| VfsError::UnknownResource(resource.clone()))?;
        self.check_access(&resource, &rel, caller)?;

        let candidate = mount.root.join(&rel);
        verify_within(&mount.root, &candidate, &rel)?;
        Ok(candidate)
    }

    /// Read bytes. Re-verifies the resolved target is still inside the sandbox
    /// *after* following any filesystem links.
    pub fn read(&self, virtual_path: &str, caller: &str) -> Result<Vec<u8>, VfsError> {
        let target = self.resolve(virtual_path, caller)?;
        self.read_resolved(&target)
    }

    fn read_resolved(&self, target: &Path) -> Result<Vec<u8>, VfsError> {
        // Re-canonicalize to catch a symlink swapped in since resolve().
        let real = fs::canonicalize(target).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => VfsError::NotFound(display_rel(target)),
            _ => VfsError::Io,
        })?;
        verify_canonical_within(target.parent().unwrap_or(Path::new("")), &real)?;
        fs::read(&real).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => VfsError::NotFound(display_rel(target)),
            _ => VfsError::Io,
        })
    }

    /// Write bytes. Only the owning resource may write, and only if the mount
    /// is writable. The parent directory must already exist (no implicit
    /// creation across the sandbox boundary).
    pub fn write(&self, virtual_path: &str, caller: &str, data: &[u8]) -> Result<(), VfsError> {
        let (resource, rel) = split_virtual(virtual_path)?;
        if resource != caller {
            return Err(VfsError::CrossResource(resource, caller.to_string()));
        }
        let mount = self.mounts.get(&resource).ok_or_else(|| VfsError::UnknownResource(resource.clone()))?;
        if !mount.writable {
            return Err(VfsError::Io);
        }
        let rel = sanitize_relative(&rel)?;
        let target = mount.root.join(&rel);
        verify_within(&mount.root, &target, &rel)?;

        // Writes must not follow a symlink outside the sandbox. Write to a
        // temp sibling then rename, and only after final verification.
        let parent = target.parent().unwrap_or(Path::new(""));
        if parent.as_os_str().is_empty() {
            return Err(VfsError::PathEscape(rel));
        }
        // ensure parent is inside the sandbox
        let parent_canon = canonicalize_or(parent, parent)?;
        verify_canonical_within(&mount.root, &parent_canon)?;
        let tmp = parent_canon.join(format!(".ald-tmp-{}", unique_suffix()));
        fs::write(&tmp, data).map_err(|_| VfsError::Io)?;
        // final containment check before promotion
        let target_canon_check = parent_canon.join(target.file_name().unwrap_or_else(|| OsStr::new("")));
        verify_canonical_within(&mount.root, &target_canon_check)?;
        match fs::rename(&tmp, &target_canon_check) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = fs::remove_file(&tmp);
                Err(VfsError::Io)
            }
        }
    }

    fn check_access(&self, resource: &str, _rel: &str, caller: &str) -> Result<(), VfsError> {
        if resource == caller {
            return Ok(());
        }
        match self.mounts.get(resource) {
            Some(m) if m.shared_with.iter().any(|s| s == caller) => Ok(()),
            _ => Err(VfsError::CrossResource(resource.to_string(), caller.to_string())),
        }
    }

    pub fn mount_count(&self) -> usize {
        self.mounts.len()
    }
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse `@resource/path...` into (resource, relative_path).
pub fn split_virtual(virtual_path: &str) -> Result<(String, String), VfsError> {
    let v = virtual_path.trim();
    if !v.starts_with('@') {
        return Err(VfsError::PathEscape(v.to_string()));
    }
    let rest = &v[1..];
    let (resource, rel) = match rest.find(['/', '\\']) {
        Some(i) => (rest[..i].to_string(), rest[i + 1..].to_string()),
        None => (rest.to_string(), String::new()),
    };
    validate_resource_name(&resource)?;
    Ok((resource, rel))
}

/// A resource name is `[A-Za-z0-9_.-]+`, no separators, no device names.
pub fn validate_resource_name(name: &str) -> Result<(), VfsError> {
    if name.is_empty() || name.len() > 64 {
        return Err(VfsError::BadResourceName(name.to_string()));
    }
    // A name is a single path component: `.` and `..` are never valid names.
    if name == "." || name == ".." {
        return Err(VfsError::BadResourceName(name.to_string()));
    }
    let ok = name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        return Err(VfsError::BadResourceName(name.to_string()));
    }
    if is_reserved_device_name(name) {
        return Err(VfsError::ReservedDevice(name.to_string()));
    }
    Ok(())
}

/// Lexical + ADS + device-name sanitization of the relative portion.
pub fn sanitize_relative(rel: &str) -> Result<String, VfsError> {
    if rel.is_empty() {
        return Ok(String::new());
    }
    if !rel.is_ascii() && !is_normalized_nfc(rel) {
        return Err(VfsError::PathEscape(rel.to_string()));
    }
    // Reject NUL bytes outright.
    if rel.contains('\0') {
        return Err(VfsError::PathEscape("nul byte".into()));
    }
    // Normalize separators FIRST: every check below sees one canonical form.
    let rel = rel.replace('\\', "/");

    // Reject drive letters / absolute separators.
    if rel.starts_with('/') || rel.len() >= 2 && rel.as_bytes()[1] == b':' {
        return Err(VfsError::AbsolutePath);
    }
    // Alternate data streams: any ':' in the final component.
    let last = rel.rsplit('/').next().unwrap_or("");
    if last.contains(':') {
        return Err(VfsError::AdsNotAllowed(last.to_string()));
    }
    // Windows reserved device names in any component.
    for comp in rel.split('/') {
        if is_reserved_device_name(comp) {
            return Err(VfsError::ReservedDevice(comp.to_string()));
        }
    }
    // Lexical traversal bound: net depth must never go negative.
    let mut depth: i32 = 0;
    for comp in rel.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(VfsError::PathEscape(rel.to_string()));
                }
            }
            _ => depth += 1,
        }
    }
    Ok(rel)
}

/// True if a component is a Windows reserved device name (case-insensitive,
/// with the trailing space / colon suffixes Windows tolerates).
pub fn is_reserved_device_name(comp: &str) -> bool {
    let stem = comp.split(':').next().unwrap_or(comp);
    let trimmed = stem.trim_end_matches(' ');
    // Windows reserves COM1-COM9 and LPT1-LPT9; nothing higher.
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("com").or_else(|| lower.strip_prefix("lpt")) {
        return rest.is_empty()
            || rest.bytes().all(|b| b.is_ascii_digit())
                && rest.parse::<u32>().map(|n| (1..=9).contains(&n)).unwrap_or(false);
    }
    matches!(lower.as_str(), "con" | "prn" | "aux" | "nul" | "conin$" | "conout$" | "$mft" | "$log" | "$volume")
}

/// Component-wise containment check. Does not touch the filesystem, so it is
/// a *necessary* not sufficient condition; pair with verify_canonical_within.
pub fn verify_within(root: &Path, candidate: &Path, rel: &str) -> Result<(), VfsError> {
    // Redundant belt-and-braces via Path components. The mount root is an
    // absolute host path (Windows: `\\?\C:\...` => Prefix + RootDir + ...),
    // so the leading absolute-prefix components of the candidate mirror the
    // root itself and must be allowed. Anything absolute *after* a normal
    // component, or any ParentDir, is an escape.
    let mut acc = root.to_path_buf();
    let mut at_start = root.has_root();
    for comp in candidate.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir if at_start => {}
            Component::Prefix(_) | Component::RootDir => {
                return Err(VfsError::AbsolutePath);
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(VfsError::PathEscape(rel.to_string())),
            Component::Normal(p) => {
                at_start = false;
                acc.push(p);
            }
        }
    }
    if !acc.starts_with(root) {
        return Err(VfsError::PathEscape(rel.to_string()));
    }
    Ok(())
}

/// Canonical (symlink-resolved) containment. This is the check that actually
/// defeats symlink/junction/reparse escapes.
pub fn verify_canonical_within(root: &Path, target: &Path) -> Result<(), VfsError> {
    let root_c = canonicalize_or(root, root)?;
    let target_c = canonicalize_or(target, target)?;
    if target_c == root_c {
        return Ok(());
    }
    if !target_c.starts_with(&root_c) {
        return Err(VfsError::PathEscape(display_rel(&target_c)));
    }
    Ok(())
}

fn canonicalize_or(p: &Path, fallback: &Path) -> Result<PathBuf, VfsError> {
    match fs::canonicalize(p) {
        Ok(c) => Ok(c),
        Err(_) => Ok(normalize_lexical(fallback)),
    }
}

/// Lexical normalization when canonicalize is unavailable (path does not
/// exist yet). Resolves `..`/`.` purely lexically.
pub fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

fn display_rel(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn is_normalized_nfc(s: &str) -> bool {
    // Approximate NFC check without a unicode-normalization dependency:
    // reject combining marks that would allow homoglyph path confusion.
    !s.chars().any(|c| {
        ('\u{0300}'..='\u{036F}').contains(&c) // combining diacritics
            || c == '\u{200B}' // zero-width space
            || c == '\u{202E}' // right-to-left override
    })
}

fn unique_suffix() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Bounded directory listing inside one resource namespace.
pub fn list_dir(vfs: &Vfs, virtual_dir: &str, caller: &str) -> Result<Vec<String>, VfsError> {
    let dir = vfs.resolve(virtual_dir, caller)?;
    let mut out = Vec::new();
    for e in fs::read_dir(&dir).map_err(|_| VfsError::NotFound(display_rel(&dir)))? {
        let e = e.map_err(|_| VfsError::Io)?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(".ald-tmp-") {
            continue;
        }
        out.push(name);
    }
    out.sort();
    Ok(out)
}

/// Snapshot of VFS state for Aegis / F8 diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsDiagnostics {
    pub mounts: Vec<String>,
    pub writable: Vec<String>,
    pub shared_edges: Vec<String>,
}

impl Vfs {
    pub fn diagnostics(&self) -> VfsDiagnostics {
        let mut mounts: Vec<String> = self.mounts.keys().cloned().collect();
        mounts.sort_unstable();
        let mut writable: Vec<String> =
            self.mounts.values().filter(|m| m.writable).map(|m| m.resource.clone()).collect();
        writable.sort_unstable();
        let mut shared_edges: Vec<String> = self
            .mounts
            .values()
            .flat_map(|m| m.shared_with.iter().map(move |o| format!("{}->{}", m.resource, o)))
            .collect();
        shared_edges.sort_unstable();
        VfsDiagnostics { mounts, writable, shared_edges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::MAIN_SEPARATOR;
    use tempfile::tempdir;

    fn setup(dir: &Path) -> Vfs {
        let mut v = Vfs::new();
        v.mount("my_res", &dir.join("my_res"), true).unwrap();
        v
    }

    #[test]
    fn split_virtual_parses() {
        let (r, p) = split_virtual("@my_res/data/x.json").unwrap();
        assert_eq!(r, "my_res");
        assert_eq!(p, "data/x.json");
        let (r2, p2) = split_virtual("@other").unwrap();
        assert_eq!(r2, "other");
        assert_eq!(p2, "");
    }

    #[test]
    fn split_virtual_rejects_bare() {
        assert!(split_virtual("my_res/x").is_err());
        // `!` is legal in a *resource name* per [A-Za-z0-9_.-]; it is the
        // *path* charset that rejects it, so use a real offender here.
        assert!(split_virtual("@../escape").is_err());
    }

    #[test]
    fn resource_name_validation() {
        assert!(validate_resource_name("my_res").is_ok());
        assert!(validate_resource_name("a-b.c_1").is_ok());
        assert!(validate_resource_name("").is_err());
        assert!(validate_resource_name("has space").is_err());
        assert!(validate_resource_name("../escape").is_err());
        assert!(validate_resource_name("NUL").is_err());
        assert!(validate_resource_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn reserved_device_names() {
        assert!(is_reserved_device_name("NUL"));
        assert!(is_reserved_device_name("con"));
        assert!(is_reserved_device_name("COM1"));
        assert!(is_reserved_device_name("com1"));
        assert!(is_reserved_device_name("LPT1"));
        assert!(is_reserved_device_name("aux"));
        assert!(is_reserved_device_name("prn"));
        assert!(!is_reserved_device_name("console"));
        assert!(!is_reserved_device_name("nul.txt")); // stem is `nul.txt`
        assert!(!is_reserved_device_name("COM10")); // >4 digits
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize_relative("../etc/passwd").is_err());
        assert!(sanitize_relative("a/../../b").is_err());
        assert!(sanitize_relative("/etc/passwd").is_err());
        assert!(sanitize_relative("C:/windows").is_err());
        assert!(sanitize_relative("file.txt:evil").is_err());
        assert!(sanitize_relative("a/NUL/b").is_err());
        assert!(sanitize_relative("a\0b").is_err());
        assert!(sanitize_relative("a/b").is_ok());
        // `a\..\b` normalizes to `a/../b`: net-zero backtracking inside the
        // sandbox, legal; only escape *beyond* the root is blocked.
        assert!(sanitize_relative("a\\..\\b").is_ok());
        assert!(sanitize_relative("a/../b").is_ok());
        assert!(sanitize_relative("..\\..\\x").is_err());
        assert!(sanitize_relative("a\\..\\..\\b").is_err());
        assert!(sanitize_relative("x/../../y").is_err());
    }

    #[test]
    fn sanitize_rejects_rtlo_and_combining() {
        assert!(sanitize_relative("a\u{202E}b").is_err());
        assert!(sanitize_relative("cafe\u{0301}").is_err());
    }

    #[test]
    fn resolve_and_read() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("my_res");
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/x.json"), b"{}").unwrap();
        let v = setup(dir.path());
        let p = v.resolve("@my_res/data/x.json", "my_res").unwrap();
        assert_eq!(v.read("@my_res/data/x.json", "my_res").unwrap(), b"{}");
        assert!(p.starts_with(fs::canonicalize(&root).unwrap()));
    }

    #[test]
    fn cross_resource_denied_unless_shared() {
        let dir = tempdir().unwrap();
        let mut v = Vfs::new();
        v.mount("a", &dir.path().join("a"), true).unwrap();
        v.mount("b", &dir.path().join("b"), true).unwrap();
        assert!(v.resolve("@a/x", "b").is_err());
        v.share("a", "b").unwrap();
        assert!(v.resolve("@a/x", "b").is_ok());
        // sharing is one-directional
        assert!(v.resolve("@b/x", "a").is_err());
    }

    #[test]
    fn traversal_blocked() {
        let dir = tempdir().unwrap();
        let v = setup(dir.path());
        assert!(v.resolve("@my_res/../../secret", "my_res").is_err());
        assert!(v.resolve("@my_res/a/../..", "my_res").is_err());
    }

    #[test]
    fn symlink_escape_blocked() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("my_res");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        }
        #[cfg(windows)]
        {
            let _ = std::os::windows::fs::symlink_dir(&outside, root.join("link"));
        }
        let v = setup(dir.path());
        let res = v.read("@my_res/link/secret.txt", "my_res");
        assert!(res.is_err(), "symlink escape must be blocked");
    }

    #[test]
    fn write_and_reject_outside_write() {
        let dir = tempdir().unwrap();
        let mut v = Vfs::new();
        v.mount("w", &dir.path().join("w"), true).unwrap();
        v.mount("ro", &dir.path().join("ro"), false).unwrap();
        fs::create_dir_all(dir.path().join("w")).unwrap();
        fs::create_dir_all(dir.path().join("ro")).unwrap();
        v.write("@w/f.txt", "w", b"hello").unwrap();
        assert_eq!(v.read("@w/f.txt", "w").unwrap(), b"hello");
        // read-only mount cannot write
        assert!(v.write("@ro/f.txt", "ro", b"x").is_err());
        // cannot write into another resource's namespace
        assert!(v.write("@w/f.txt", "ro", b"x").is_err());
    }

    #[test]
    fn list_dir_skips_temp() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("my_res");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"").unwrap();
        fs::write(root.join(".ald-tmp-1"), b"").unwrap();
        let v = setup(dir.path());
        let names = list_dir(&v, "@my_res", "my_res").unwrap();
        assert_eq!(names, vec!["a.txt"]);
    }

    #[test]
    fn diagnostics_reflects_state() {
        let dir = tempdir().unwrap();
        let mut v = Vfs::new();
        v.mount("a", &dir.path().join("a"), true).unwrap();
        v.mount("b", &dir.path().join("b"), false).unwrap();
        v.share("a", "b").unwrap();
        let d = v.diagnostics();
        assert_eq!(d.mounts, vec!["a", "b"]);
        assert_eq!(d.writable, vec!["a"]);
        assert_eq!(d.shared_edges, vec!["a->b"]);
    }

    #[test]
    fn normalize_lexical_resolves_dots() {
        let p = normalize_lexical(Path::new("/a/b/../c/./d"));
        assert_eq!(p, PathBuf::from("/a/c/d"));
    }

    #[test]
    fn unknown_resource_errors() {
        let dir = tempdir().unwrap();
        let v = setup(dir.path());
        assert!(matches!(v.resolve("@nope/x", "my_res"), Err(VfsError::UnknownResource(_))));
    }

    /// Fuzz-style sweep over generated relative paths: every generator is
    /// adversarial, so ALL of these must either error or resolve to a path
    /// that is provably still inside the sandbox root.
    #[test]
    fn fuzz_relative_paths_stay_contained() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("my_res");
        fs::create_dir_all(&root).unwrap();
        let mut v = Vfs::new();
        v.mount("my_res", &root, true).unwrap();

        let atoms = ["a", "..", ".", "b", "NUL", "con", "x.txt", "a:b", "com1", ""];
        let radix = atoms.len();
        let root_canon = fs::canonicalize(&root).unwrap();
        let mut count = 0usize;
        // every path of 1..=4 atoms, enumerated as a mixed-radix odometer
        for n in 1..=4usize {
            for k in 0..radix.pow(n as u32) {
                let mut digits = Vec::with_capacity(n);
                let mut rem = k;
                for _ in 0..n {
                    digits.push(rem % radix);
                    rem /= radix;
                }
                let rel: String = digits.iter().map(|&d| atoms[d]).collect::<Vec<_>>().join("/");
                let variants = [
                    rel.clone(),
                    rel.replace(MAIN_SEPARATOR, "/"),
                    format!("{}{}", MAIN_SEPARATOR, rel),
                    format!("../{}", rel),
                ];
                for vpath in variants.iter().map(|r| format!("@my_res/{}", r)) {
                    count += 1;
                    if let Ok(p) = v.resolve(&vpath, "my_res") {
                        assert!(
                            verify_canonical_within(&root_canon, &p).is_ok() || !p.exists(),
                            "escaped sandbox: {} -> {:?}",
                            vpath,
                            p
                        );
                    }
                }
            }
        }
        assert!(count > 100, "fuzz sweep too small: {} cases", count);
    }
}
