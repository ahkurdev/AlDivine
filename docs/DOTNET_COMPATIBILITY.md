# DOTNET COMPATIBILITY

Status: **PARTIAL** — load-and-authorize half IMPLEMENTED, CIL execution
BLOCKED_EXTERNAL.

Embedding an arbitrary modern .NET and calling it CfxCLR compatibility would
be exactly the placeholder compatibility the master spec forbids. FiveM-era
resources run against Mono/CfxCLR semantics: `BaseScript` subclasses started
on resource start, `EventHandlers`/`Tick`/`Delay`, `exports`, `Player` /
`Players`, native wrappers, assembly dependency loading, debug symbols. None
of that can be claimed without a CLR in the process.

`crates/ald-script-dotnet` is the separate compatibility runtime for .NET
resources. It implements everything that can be implemented without an
engine: real assembly parsing, dependency resolution, and resource
authorization. It executes nothing.

## What is implemented

| Area | Status | Detail |
|------|--------|--------|
| ECMA-335 metadata reader | IMPLEMENTED | Real PE → CLI header → metadata streams → tables walk over on-disk assemblies. Recovers runtime version, CLI flags, module name, MVID, assembly identity, assembly references, TypeDef list with base types, P/Invoke imports, custom-attribute type names. Verified against genuine framework assemblies (`mscorlib.dll`, `System.Core.dll`), not synthetic bytes. |
| BCL target inference | IMPLEMENTED | `System.Private.CoreLib` / `System.Runtime` → CoreClr; `mscorlib` → NetFramework; Mono markers → Mono; else Unknown. A Framework-built resource is refused on a CoreCLR host instead of pretending to load. |
| CLR host profiles | IMPLEMENTED | `Mono` (FiveM-era) and `CoreClr` (new deployments), selected from normalized manifest `clr_disable_task_scheduler` via `ClrProfile::from_manifest`. |
| Dependency resolution | IMPLEMENTED | Transitive `AssemblyRef` matching by simple name, version downgrade refused, higher version recorded as Upgrade (never hidden), every non-null token checked against an explicit `TrustList`, deterministic topological load plan, cycle reported with members. |
| Resource authorization | IMPLEMENTED | `analyze_resource` gates: BCL mismatch, no `CitizenFX.Core` reference, no `BaseScript`-derived type, unsigned assembly (default off), unmanaged P/Invoke imports (default off, first import named). Accepted resources report script types, Cfx attribute types, target, and native imports. |
| Identity tokens | IMPLEMENTED | Public-key-token derivation documented as SHA-1-tail-reversed against an independently checkable vector; signature verification itself NOT claimed (needs RSA + assembly self-hash). |
| Tick ordering semantics | IMPLEMENTED | `TickPump` virtual-clock BaseScript tick boundaries (30 ms catch-up), pinned by tests so engine integration is wiring, not design. |
| CIL execution | BLOCKED_EXTERNAL | No CLR is embedded in this build. No method body ever runs, no `Tick` loop fires. |

## BLOCKED_EXTERNAL — exact blocker

Running a resource method requires a hosted CLR (Mono for FiveM-era
semantics, CoreCLR for modern) with BaseScript lifetime, task scheduler
(`clr_disable_task_scheduler` honored), event/tick dispatch, and native
wrappers — plus, for compatibility claims, the ability to run the same
corpus under both flavors.

Blockers, precisely:

1. **No CLR in the build.** The workspace embeds no Mono, CoreCLR, or
   .NET Framework host, and `dotnet` is absent offline. This crate has no
   engine dependency by design.
2. **Method-level binding needs a deeper walk.** `[EventHandler]` on a
   specific method needs the MethodSemantics/attribute-blob walk this
   reader does not perform; the analyzer reports attribute *types*
   present, honestly, not per-method bindings.
3. **Flavor parity is unverifiable without both runtimes.** Claiming
   "Mono PASS / CoreCLR PASS" requires actually running the corpus on
   both. Neither runtime is present, so no such claim is made.

Per the master spec, the blocked part is: isolated (pure metadata crate),
abstracted (resolution/authorization behind `Loaded` / `TrustList` /
`ClrPolicy`), test-doubled (hand-built `CliImage` fixtures plus real
framework DLLs as parse-only fixtures), and documented here — then
independent work continued.

## Consequence for resources

A .NET resource is **NOT_SUPPORTED** for execution today. It is analyzed
and either accepted-for-future-execution or rejected with a specific
reason (`BclMismatch`, `NotResource`, `Unsigned`, `NativeCall`); it is
never silently run, never run partially, never marked compatible.

## Test evidence

`cargo test -p ald-script-dotnet` — 20 tests. The set that matters:

- `parses_real_mscorlib_identity` — genuine `mscorlib.dll` parses to the
  well-known identity and runtime version.
- `system_core_references_mscorlib_with_matching_token` — genuine
  `System.Core.dll` references `mscorlib` 4.0.0.0 token b77a5c561934e089.
- `real_framework_assembly_is_rejected_as_resource` — the same genuine
  assembly is rejected as a resource for specific reasons, never accepted.
- `accepts_a_wellformed_cfxclr_resource` — CitizenFX.Core + System.Runtime
  + BaseScript subclass + signed + no P/Invoke is accepted with target coreclr.
- `rejects_native_calls_and_names_the_first_import` — kernel32!GetTickCount
  refused by default, named, and reported when permitted.
- `load_plan_orders_dependencies_before_dependents` — topological order with
  the version upgrade recorded, not hidden.
- `load_plan_refuses_downgrade_missing_and_untrusted` — downgrade, missing,
  and untrusted-token cases each fail distinctly.
- `truncation_sweep_never_panics` — every prefix of a real assembly errors,
  never panics.

## Upgrade path

To move execution out of BLOCKED_EXTERNAL:

1. Embed a real CLR (Mono for legacy semantics first; CoreCLR as a second
   profile) behind a process boundary — a resource crash must not take the
   server down.
2. Implement BaseScript lifetime, task scheduler, EventHandlers/Tick/Delay,
   exports, Player/Players, and native wrappers against this crate's
   `ResourceAnalysis`, enforcing `ClrPolicy` at load time.
3. Run one authored corpus under both flavors and record the results in
   `docs/compatibility/DOTNET_MATRIX.md` as exact-version certification.
4. Only then may a "Mono PASS / CoreCLR PASS" row appear anywhere.
