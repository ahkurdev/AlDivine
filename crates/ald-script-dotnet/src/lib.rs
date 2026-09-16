//! Aldivine CfxCLR / .NET compatibility runtime — the load-and-authorize half.
//!
//! What this crate actually is:
//!
//! * A **CLI metadata layer** over [`ecma335`]: real managed assemblies on
//!   disk are parsed (PE → CLI header → metadata streams → tables) without
//!   executing anything. This is verified against actual framework
//!   assemblies on the host (`mscorlib.dll`, `System.Core.dll`), not against
//!   hand-made bytes pretending to be ones.
//! * A **dependency resolver**: given a set of loaded assemblies, every
//!   `AssemblyRef` is matched by name, versioned against the request
//!   (downgrade refused), and checked against an explicit trust list of
//!   public key tokens. Output is a topologically ordered load plan with
//!   cycle detection.
//! * A **CfxCLR resource analyzer**: decides whether an assembly can act as
//!   an Aldivine .NET resource — it must target the selected CLR profile,
//!   reference `CitizenFX.Core`, declare at least one `BaseScript` subclass,
//!   and satisfy the host's pinvoke/signature policy.
//! * **Profile selection** from the normalized manifest
//!   (`clr_disable_task_scheduler`) and a BCL-target check
//!   (Framework-vs-CoreCLR-vs-Mono from the reference set).
//!
//! What this crate is NOT, deliberately:
//!
//! * It does not host a CLR. There is no Mono, CoreCLR, or .NET Framework
//!   embedding in this build, so no CIL ever executes and no `EventHandlers`
//!   tick loop runs. Execution is **BLOCKED_EXTERNAL** (see
//!   `docs/DOTNET_COMPATIBILITY.md`). Claiming "BaseScript Tick supported"
//!   without an engine would be exactly the placeholder compatibility the
//!   master spec forbids.
//! * It does not verify strong-name *signatures* (that needs RSA + the
//!   assembly's hash of its own metadata). It verifies *identity tokens*
//!   against an explicit trust list and says so plainly.
//! * Method-level attribute binding (`[EventHandler]` on a specific method)
//!   needs the MethodSemantics/AttrBlob walk this reader does not perform;
//!   the analyzer reports attribute *types* present in the assembly and is
//!   honest in the matrix about what is and isn't resolved per-method.

pub mod ecma335;

use ecma335::{AssemblyIdentity, CliError, CliImage, Pinvoke, TargetFramework};
use std::collections::HashMap;

pub use ecma335::read_assembly;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClrError {
    #[error("{0}")]
    Cli(#[from] CliError),
    #[error("assembly `{0}` is unsigned and unsigned_resource_assemblies is not permitted")]
    Unsigned(String),
    #[error("assembly `{assembly}` declares {count} unmanaged import(s) ({first}) but native_calls are not permitted")]
    NativeCall { assembly: String, count: usize, first: String },
    #[error("dependency `{reference}` of `{from}` not found among loaded assemblies")]
    MissingDependency { from: String, reference: String },
    #[error("assembly `{reference}` (version {found}) is older than requested version {requested} by `{from}`")]
    VersionDowngrade { reference: String, found: String, requested: String, from: String },
    #[error("public key token `{token}` for `{reference}` is not in the trust list")]
    Untrusted { reference: String, token: String },
    #[error("dependency cycle among assemblies: {0}")]
    Cycle(String),
    #[error("not a CfxCLR resource: {0}")]
    NotResource(String),
    #[error("assembly targets {target} but the host profile runs {host}")]
    BclMismatch { target: String, host: String },
}

// ---------------------------------------------------------------------------
// CLR host profile
// ---------------------------------------------------------------------------

/// Which CLR the host process would embed. FiveM-era resources run on Mono;
/// Aldivine targets CoreCLR for new deployments. A resource compiled against
/// one BCL does not load unchanged on the other, and this crate refuses the
/// mismatch instead of pretending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClrFlavor {
    Mono,
    CoreClr,
}

impl ClrFlavor {
    pub fn as_str(self) -> &'static str {
        match self {
            ClrFlavor::Mono => "mono",
            ClrFlavor::CoreClr => "coreclr",
        }
    }

    /// Which BCL target an assembly must declare to load on this host.
    pub fn accepts(self, target: TargetFramework) -> bool {
        match self {
            ClrFlavor::Mono => matches!(target, TargetFramework::NetFramework | TargetFramework::Mono),
            ClrFlavor::CoreClr => matches!(target, TargetFramework::CoreClr),
        }
    }
}

/// Host configuration for the CLR compatibility profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ClrProfile {
    pub flavor: ClrFlavor,
    /// `clr_disable_task_scheduler` from the manifest: when true the legacy
    /// BaseScript task scheduler must not be started (Cfx behaviour).
    pub disable_task_scheduler: bool,
}

impl Default for ClrProfile {
    fn default() -> Self {
        ClrProfile { flavor: ClrFlavor::CoreClr, disable_task_scheduler: false }
    }
}

impl ClrProfile {
    /// Derive the profile from a normalized fxmanifest. Only the CLR-relevant
    /// directives are consumed; everything else stays the manifest's problem.
    pub fn from_manifest(m: &ald_fxmanifest::NormalizedManifest) -> ClrProfile {
        ClrProfile { disable_task_scheduler: m.clr_disable_task_scheduler, ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// Loaded assembly + dependency resolution
// ---------------------------------------------------------------------------

/// One parsed assembly plus its bytes-level identity for diagnostics.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub image: CliImage,
    pub path: String,
}

impl Loaded {
    /// Parse bytes from a known path. Errors are the reader's errors.
    pub fn load(path: impl Into<String>, bytes: &[u8]) -> Result<Loaded, ClrError> {
        Ok(Loaded { image: read_assembly(bytes)?, path: path.into() })
    }

    pub fn identity(&self) -> Option<&AssemblyIdentity> {
        self.image.assembly.as_ref()
    }

    /// Simple name used as the resolution key (`CitizenFX.Core`).
    pub fn name(&self) -> &str {
        self.identity().map(|a| a.name.as_str()).unwrap_or("")
    }
}

/// Outcome of matching one `AssemblyRef` against the loaded set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Exact version match and trusted token.
    Ok { version: String, trusted: bool },
    /// Loaded version is newer than requested (binding redirects upward).
    Upgraded { requested: String, found: String },
}

/// An explicit trust list of public key tokens (lowercase hex).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustList {
    tokens: Vec<String>,
}

impl TrustList {
    pub fn new(tokens: impl IntoIterator<Item = impl Into<String>>) -> TrustList {
        TrustList { tokens: tokens.into_iter().map(Into::into).collect() }
    }
    pub fn contains(&self, token: &str) -> bool {
        self.tokens.iter().any(|t| t == token)
    }
}

/// Version comparison on the four CLI parts; lexicographic on tuples.
fn version_tuple(v: &str) -> [u32; 4] {
    let mut out = [0u32; 4];
    for (i, p) in v.split('.').take(4).enumerate() {
        out[i] = p.parse().unwrap_or(0);
    }
    out
}

fn satisfies(requested: &str, found: &str) -> bool {
    version_tuple(found) >= version_tuple(requested)
}

/// The load plan for one root assembly, transitive over AssemblyRef.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadPlan {
    /// Assembly names in dependency order (dependencies first).
    pub order: Vec<String>,
    /// Per (requesting assembly, reference) resolution outcome.
    pub edges: Vec<(String, String, Resolved)>,
}

/// Resolve `root`'s transitive dependencies over `set`.
///
/// Rules: a reference must exist by simple name; a *lower* found version is a
/// hard error (no silent downgrade); a *higher* found version is an Upgrade
/// (the host would need a binding redirect, recorded not hidden); every
/// non-null token must appear in `trust`, or resolution fails. The order is a
/// deterministic topological sort; cycles are reported with their members.
pub fn plan_load(set: &HashMap<String, Loaded>, root: &str, trust: &TrustList) -> Result<LoadPlan, ClrError> {
    let mut edges = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut state: HashMap<String, u8> = HashMap::new(); // 0 unseen, 1 in stack, 2 done

    fn dfs(
        name: &str,
        set: &HashMap<String, Loaded>,
        trust: &TrustList,
        state: &mut HashMap<String, u8>,
        order: &mut Vec<String>,
        edges: &mut Vec<(String, String, Resolved)>,
    ) -> Result<(), ClrError> {
        match state.get(name).copied().unwrap_or(0) {
            2 => return Ok(()),
            1 => {
                let cycle = order
                    .iter()
                    .skip_while(|n| n.as_str() != name)
                    .cloned()
                    .chain(std::iter::once(name.to_string()))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(ClrError::Cycle(cycle));
            }
            _ => {}
        }
        state.insert(name.to_string(), 1);
        let node = set
            .get(name)
            .ok_or_else(|| ClrError::MissingDependency { from: "(root)".to_string(), reference: name.to_string() })?;
        let from = node.name().to_string();
        for r in &node.image.references {
            let found = set
                .get(&r.name)
                .ok_or_else(|| ClrError::MissingDependency { from: from.clone(), reference: r.name.clone() })?;
            let fv = found.identity().map(|a| a.version.clone()).unwrap_or_default();
            if !satisfies(&r.version, &fv) {
                return Err(ClrError::VersionDowngrade {
                    reference: r.name.clone(),
                    found: fv,
                    requested: r.version.clone(),
                    from: from.clone(),
                });
            }
            if let Some(tok) = &r.public_key_token {
                if !trust.contains(tok) {
                    return Err(ClrError::Untrusted { reference: r.name.clone(), token: tok.clone() });
                }
            }
            let resolved = if fv == r.version {
                Resolved::Ok { version: fv, trusted: r.public_key_token.is_some() }
            } else {
                Resolved::Upgraded { requested: r.version.clone(), found: fv }
            };
            edges.push((from.clone(), r.name.clone(), resolved));
            dfs(&r.name, set, trust, state, order, edges)?;
        }
        state.insert(name.to_string(), 2);
        order.push(name.to_string());
        Ok(())
    }

    dfs(root, set, trust, &mut state, &mut order, &mut edges)?;
    Ok(LoadPlan { order, edges })
}

// ---------------------------------------------------------------------------
// Policy gates
// ---------------------------------------------------------------------------

/// Host policy applied before any resource assembly is accepted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClrPolicy {
    /// Unmanaged imports (`DllImport`) allowed at all. Default off: the
    /// native extension ABI is the sanctioned path, raw P/Invoke from a
    /// resource assembly is not.
    pub allow_native_calls: bool,
    /// Assemblies with no public key allowed as resources. Default off.
    pub allow_unsigned_resources: bool,
    /// Tokens permitted for unsigned-but-blessed system assemblies; empty by
    /// default (no implicit trust).
    pub trust: TrustList,
}

// ---------------------------------------------------------------------------
// CfxCLR resource analysis
// ---------------------------------------------------------------------------

/// The CitizenFX API assembly every CfxCLR resource references.
pub const CITIZENFX_ASSEMBLY: &str = "CitizenFX.Core";

/// Why a method- or type-level attribute is considered Cfx-relevant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptType {
    pub full_name: String,
    pub base: String,
}

/// Result of accepting an assembly as a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAnalysis {
    pub assembly: AssemblyIdentity,
    pub target: TargetFramework,
    /// Types deriving (directly) from a `BaseScript`.
    pub script_types: Vec<ScriptType>,
    /// Names of the Cfx-relevant attribute types declared as referenced.
    pub cfx_attribute_types: Vec<String>,
    /// Unmanaged imports, reported even when permitted: operators see them.
    pub native_calls: Vec<Pinvoke>,
}

/// Decide whether `img` may act as an Aldivine .NET resource under `profile`
/// and `policy`. Every rejection is a specific, non-negotiable reason.
pub fn analyze_resource(
    img: &CliImage,
    profile: &ClrProfile,
    policy: &ClrPolicy,
) -> Result<ResourceAnalysis, ClrError> {
    let asm =
        img.assembly.as_ref().ok_or_else(|| ClrError::NotResource("module without an Assembly identity".into()))?;

    if !profile.flavor.accepts(img.target_framework()) {
        return Err(ClrError::BclMismatch {
            target: img.target_framework().as_str().to_string(),
            host: profile.flavor.as_str().to_string(),
        });
    }

    if !img.references.iter().any(|r| r.name == CITIZENFX_ASSEMBLY) {
        return Err(ClrError::NotResource(format!("no reference to {CITIZENFX_ASSEMBLY}")));
    }

    let script_types: Vec<ScriptType> = img
        .types
        .iter()
        .filter(|t| t.extends_base_script())
        .map(|t| ScriptType { full_name: t.full_name(), base: t.base.clone().unwrap_or_default() })
        .collect();

    if script_types.is_empty() {
        return Err(ClrError::NotResource("no BaseScript-derived type; nothing to start on onResourceStart".into()));
    }

    if asm.public_key_token.is_none() && !policy.allow_unsigned_resources {
        return Err(ClrError::Unsigned(asm.name.clone()));
    }

    if !img.pinvokes.is_empty() && !policy.allow_native_calls {
        let p = &img.pinvokes[0];
        return Err(ClrError::NativeCall {
            assembly: asm.name.clone(),
            count: img.pinvokes.len(),
            first: format!("{}!{}", p.module, p.function),
        });
    }

    let cfx_attribute_types =
        img.custom_attribute_types.iter().filter(|t| t.starts_with("CitizenFX")).cloned().collect();

    Ok(ResourceAnalysis {
        assembly: asm.clone(),
        target: img.target_framework(),
        script_types,
        cfx_attribute_types,
        native_calls: img.pinvokes.clone(),
    })
}

// ---------------------------------------------------------------------------
// Tick budget (virtual clock, matching the JS/Node crates)
// ---------------------------------------------------------------------------

/// BaseScript `Tick` scheduling state. With no CLR embedded these ticks never
/// fire — the pump exists so integration is a wiring job, and so the ordering
/// semantics are pinned by tests before any engine exists.
#[derive(Debug, Default, Clone)]
pub struct TickPump {
    clock_ms: u64,
    next_tick_ms: u64,
    interval_ms: u64,
    fired: u64,
}

impl TickPump {
    pub fn new(interval_ms: u64) -> TickPump {
        TickPump { interval_ms: interval_ms.max(1), next_tick_ms: 0, ..Default::default() }
    }

    /// Advance the virtual clock and return how many BaseScript ticks would
    /// have fired (Catch-up semantics: each 30 ms boundary once).
    pub fn advance(&mut self, ms: u64) -> u64 {
        self.clock_ms = self.clock_ms.saturating_add(ms);
        let mut n = 0u64;
        while self.clock_ms >= self.next_tick_ms {
            self.next_tick_ms = self.next_tick_ms.saturating_add(self.interval_ms);
            n += 1;
        }
        // next_tick_ms starts at 0 so the first boundary is immediate.
        self.fired = self.fired.saturating_add(n.saturating_sub(0));
        n.saturating_sub(0)
    }

    pub fn ticks_fired(&self) -> u64 {
        self.fired
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ecma335::{AssemblyRef, Pinvoke, TypeDefInfo};

    fn identity(name: &str, version: &str, token: Option<&str>) -> AssemblyIdentity {
        AssemblyIdentity {
            name: name.into(),
            version: version.into(),
            culture: None,
            flags: 0,
            public_key_token: token.map(str::to_string),
            has_public_key: token.is_some(),
        }
    }

    fn aref(name: &str, version: &str, token: Option<&str>) -> AssemblyRef {
        AssemblyRef {
            name: name.into(),
            version: version.into(),
            culture: None,
            public_key_token: token.map(str::to_string),
            carries_full_key: false,
        }
    }

    /// A CoreCLR-targeted CfxCLR resource: references CitizenFX.Core, derives
    /// a BaseScript, no P/Invoke.
    fn cfx_resource() -> CliImage {
        CliImage {
            runtime_version: "v4.0.30319".into(),
            clr_flags: 1,
            entry_point_token: 0,
            module_name: Some("mystuff.dll".into()),
            mvid: None,
            assembly: Some(identity("mystuff", "1.0.0.0", Some("de303ab16a3e581c"))),
            references: vec![
                aref(CITIZENFX_ASSEMBLY, "1.0.0.0", Some("de303ab16a3e581c")),
                aref("System.Runtime", "8.0.0.0", Some("b03f5f7f11d50a3a")),
            ],
            table_rows: [0u32; 64],
            custom_attribute_types: vec![
                "CitizenFX.Core.EventHandlerAttribute".into(),
                "System.Runtime.Versioning.TargetFrameworkAttribute".into(),
            ],
            types: vec![TypeDefInfo {
                namespace: "MyStaff".into(),
                name: "ServerScript".into(),
                flags: 0,
                base: Some("CitizenFX.Core.BaseScript".into()),
            }],
            pinvokes: Vec::new(),
        }
    }

    fn img_with(target_refs: Vec<AssemblyRef>, pinvokes: Vec<Pinvoke>) -> CliImage {
        CliImage { references: target_refs, pinvokes, ..cfx_resource() }
    }

    #[test]
    fn accepts_a_wellformed_cfxclr_resource() {
        let a = analyze_resource(&cfx_resource(), &ClrProfile::default(), &ClrPolicy::default())
            .expect("well-formed resource accepted");
        assert_eq!(a.script_types.len(), 1);
        assert_eq!(a.script_types[0].full_name, "MyStaff.ServerScript");
        assert_eq!(a.target, TargetFramework::CoreClr);
        assert_eq!(a.cfx_attribute_types, vec!["CitizenFX.Core.EventHandlerAttribute".to_string()]);
    }

    #[test]
    fn rejects_unsigned_resource_by_default() {
        let mut img = cfx_resource();
        img.assembly = Some(identity("mystuff", "1.0.0.0", None));
        let err = analyze_resource(&img, &ClrProfile::default(), &ClrPolicy::default()).unwrap_err();
        assert!(matches!(err, ClrError::Unsigned(_)));
        // explicitly permitted policy admits it
        let pol = ClrPolicy { allow_unsigned_resources: true, ..Default::default() };
        assert!(analyze_resource(&img, &ClrProfile::default(), &pol).is_ok());
    }

    #[test]
    fn rejects_native_calls_and_names_the_first_import() {
        let img = img_with(
            cfx_resource().references,
            vec![Pinvoke { module: "kernel32.dll".into(), function: "GetTickCount".into() }],
        );
        let err = analyze_resource(&img, &ClrProfile::default(), &ClrPolicy::default()).unwrap_err();
        assert!(matches!(&err, ClrError::NativeCall { first, .. } if first == "kernel32.dll!GetTickCount"), "{err}");
        let pol = ClrPolicy { allow_native_calls: true, ..Default::default() };
        let a = analyze_resource(&img, &ClrProfile::default(), &pol).expect("permitted");
        assert_eq!(a.native_calls.len(), 1);
    }

    #[test]
    fn rejects_assembly_without_cfx_reference() {
        let mut img = cfx_resource();
        img.references = vec![
            aref("System.Runtime", "8.0.0.0", Some("b03f5f7f11d50a3a")),
            aref("Newtonsoft.Json", "13.0.0.0", None),
        ];
        let err = analyze_resource(&img, &ClrProfile::default(), &ClrPolicy::default()).unwrap_err();
        assert!(matches!(err, ClrError::NotResource(m) if m.contains(CITIZENFX_ASSEMBLY)));
    }

    #[test]
    fn rejects_no_basescript_subclass() {
        let mut img = cfx_resource();
        img.types = vec![TypeDefInfo {
            namespace: "X".into(),
            name: "Helper".into(),
            flags: 0,
            base: Some("System.Object".into()),
        }];
        let err = analyze_resource(&img, &ClrProfile::default(), &ClrPolicy::default()).unwrap_err();
        assert!(matches!(err, ClrError::NotResource(m) if m.contains("BaseScript")));
    }

    #[test]
    fn bcl_target_must_match_host_flavor() {
        // Framework-only reference set => Mono host ok, CoreCLR host refused.
        let mut img = cfx_resource();
        img.references =
            vec![aref(CITIZENFX_ASSEMBLY, "1.0.0.0", None), aref("mscorlib", "4.0.0.0", Some("b77a5c561934e089"))];
        let err = analyze_resource(&img, &ClrProfile::default(), &ClrPolicy::default()).unwrap_err();
        assert!(matches!(err, ClrError::BclMismatch { .. }), "{err}");
        let mono = ClrProfile { flavor: ClrFlavor::Mono, disable_task_scheduler: false };
        assert!(analyze_resource(&img, &mono, &ClrPolicy::default()).is_ok());
    }

    #[test]
    fn manifest_drives_task_scheduler_flag() {
        let m = ald_fxmanifest::NormalizedManifest { clr_disable_task_scheduler: true, ..Default::default() };
        let p = ClrProfile::from_manifest(&m);
        assert!(p.disable_task_scheduler);
    }

    fn loaded(img: CliImage) -> Loaded {
        Loaded { image: img, path: String::new() }
    }

    #[test]
    fn load_plan_orders_dependencies_before_dependents() {
        let mut core = cfx_resource();
        core.assembly = Some(identity(CITIZENFX_ASSEMBLY, "2.0.0.0", Some("de303ab16a3e581c")));
        core.references = vec![aref("System.Runtime", "6.0.0.0", Some("b03f5f7f11d50a3a"))];

        let mut rt = cfx_resource();
        rt.assembly = Some(identity("System.Runtime", "6.0.0.0", Some("b03f5f7f11d50a3a")));
        rt.references = Vec::new();

        let res = loaded({
            let mut img = cfx_resource();
            img.references = vec![aref(CITIZENFX_ASSEMBLY, "1.0.0.0", Some("de303ab16a3e581c"))];
            img
        });
        let mut set: HashMap<String, Loaded> = HashMap::new();
        set.insert(CITIZENFX_ASSEMBLY.into(), loaded(core));
        set.insert("System.Runtime".into(), loaded(rt));
        set.insert("mystuff".into(), res);

        let trust = TrustList::new(["de303ab16a3e581c", "b03f5f7f11d50a3a"]);
        let plan = plan_load(&set, "mystuff", &trust).expect("plan");
        assert_eq!(plan.order, vec!["System.Runtime", CITIZENFX_ASSEMBLY, "mystuff"]);
        // requested 1.0.0.0, found 2.0.0.0 => Upgrade recorded, never hidden
        assert!(plan
            .edges
            .iter()
            .any(|(_, r, rsn)| r == CITIZENFX_ASSEMBLY && matches!(rsn, Resolved::Upgraded { .. })));
    }

    #[test]
    fn load_plan_refuses_downgrade_missing_and_untrusted() {
        let mut set: HashMap<String, Loaded> = HashMap::new();
        // CitizenFX.Core present but at 1.0.0.0
        let mut core = cfx_resource();
        core.assembly = Some(identity(CITIZENFX_ASSEMBLY, "1.0.0.0", Some("de303ab16a3e581c")));
        core.references = Vec::new();
        set.insert(CITIZENFX_ASSEMBLY.into(), loaded(core));
        let trust = TrustList::new(["de303ab16a3e581c"]);

        // downgrade: requests 2.0.0.0
        let mut big = cfx_resource();
        big.assembly = Some(identity("mystuff", "1.0.0.0", Some("de303ab16a3e581c")));
        big.references = vec![aref(CITIZENFX_ASSEMBLY, "2.0.0.0", Some("de303ab16a3e581c"))];
        set.insert("mystuff".into(), loaded(big));
        let err = plan_load(&set, "mystuff", &trust).unwrap_err();
        assert!(matches!(err, ClrError::VersionDowngrade { .. }), "{err}");

        // missing: requests Newtonsoft which is absent
        let mut miss = cfx_resource();
        miss.assembly = Some(identity("mystuff", "1.0.0.0", Some("de303ab16a3e581c")));
        miss.references = vec![aref("Newtonsoft.Json", "13.0.0.0", None)];
        set.insert("mystuff".into(), loaded(miss));
        let err = plan_load(&set, "mystuff", &trust).unwrap_err();
        assert!(matches!(err, ClrError::MissingDependency { .. }), "{err}");

        // untrusted token
        let mut mutatis = cfx_resource();
        mutatis.assembly = Some(identity("mystuff", "1.0.0.0", Some("de303ab16a3e581c")));
        mutatis.references = vec![aref(CITIZENFX_ASSEMBLY, "1.0.0.0", Some("0000000000000bad"))];
        set.insert("mystuff".into(), loaded(mutatis));
        let err = plan_load(&set, "mystuff", &trust).unwrap_err();
        assert!(matches!(err, ClrError::Untrusted { .. }), "{err}");
    }

    #[test]
    fn load_plan_detects_cycles() {
        let mut a = cfx_resource();
        a.assembly = Some(identity("a", "1.0.0.0", Some("de303ab16a3e581c")));
        a.references = vec![aref("b", "1.0.0.0", Some("de303ab16a3e581c"))];
        let mut b = cfx_resource();
        b.assembly = Some(identity("b", "1.0.0.0", Some("de303ab16a3e581c")));
        b.references = vec![aref("a", "1.0.0.0", Some("de303ab16a3e581c"))];
        let mut set: HashMap<String, Loaded> = HashMap::new();
        set.insert("a".into(), loaded(a));
        set.insert("b".into(), loaded(b));
        let trust = TrustList::new(["de303ab16a3e581c"]);
        let err = plan_load(&set, "a", &trust).unwrap_err();
        assert!(matches!(err, ClrError::Cycle(_)), "{err}");
    }

    #[test]
    fn tick_pump_fires_on_interval_boundaries() {
        let mut pump = TickPump::new(30);
        assert_eq!(pump.advance(0), 1, "t=0 boundary fires once");
        assert_eq!(pump.advance(29), 0);
        assert_eq!(pump.advance(1), 1, "30ms boundary");
        assert_eq!(pump.advance(60), 2, "60 and 90 boundaries");
        assert_eq!(pump.ticks_fired(), 4);
    }

    /// End-to-end through the real parser: a genuine framework assembly must
    /// be rejected as a resource for specific reasons, never accepted.
    #[test]
    fn real_framework_assembly_is_rejected_as_resource() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let p = std::path::PathBuf::from(windir)
            .join("Microsoft.NET")
            .join("Framework64")
            .join("v4.0.30319")
            .join("mscorlib.dll");
        let Ok(bytes) = std::fs::read(&p) else {
            eprintln!("skip: {} absent", p.display());
            return;
        };
        let img = read_assembly(&bytes).expect("mscorlib parses");
        let err = analyze_resource(&img, &ClrProfile::default(), &ClrPolicy::default())
            .expect_err("mscorlib is not a CfxCLR resource");
        // It's unsigned-by-policy irrelevant; the decisive gates are BCL
        // mismatch on a CoreCLR host, or no CitizenFX reference.
        assert!(
            matches!(err, ClrError::BclMismatch { .. } | ClrError::NotResource(_) | ClrError::Unsigned(_)),
            "{err}"
        );
    }

    #[test]
    fn version_tuple_parses_partial_versions() {
        assert_eq!(version_tuple("1.0"), [1, 0, 0, 0]);
        assert_eq!(version_tuple("4.0.0.0"), [4, 0, 0, 0]);
        assert_eq!(version_tuple("junk"), [0, 0, 0, 0]);
        assert!(satisfies("1.0.0.0", "2.0.0.0"));
        assert!(!satisfies("2.0.0.1", "2.0.0.0"));
    }
}
