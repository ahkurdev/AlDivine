# DOTNET MATRIX

Exact-version certification for the Aldivine CfxCLR Compatibility Runtime.

**No row in this file is PASS yet.** A row may only be marked PASS with the
engine version, Aldivine version, the corpus used, and a date — and only when
that run actually happened. See `docs/DOTNET_COMPATIBILITY.md` for the
BLOCKED_EXTERNAL blocker.

## Certification rows

| Profile | Engine | Aldivine | Corpus | Result | Date |
|---------|--------|----------|--------|--------|------|
| mono (FiveM-era) | not embedded | 0.1.0 | — | BLOCKED_EXTERNAL | — |
| coreclr (modern) | not embedded | 0.1.0 | — | BLOCKED_EXTERNAL | — |

## What is verified today (engine-independent)

These are tested behaviours of `crates/ald-script-dotnet`, each with a test in
that crate. They are compatibility *machinery*, not CLR compatibility claims.

| Behaviour | Status |
|-----------|--------|
| Real `mscorlib.dll` parses to known identity | PASS |
| Real `System.Core.dll` references mscorlib with matching token | PASS |
| Real framework assembly rejected as resource (specific reason) | PASS |
| Well-formed CfxCLR image accepted with target coreclr | PASS |
| Missing CitizenFX.Core reference rejected, assembly named | PASS |
| No BaseScript subclass rejected | PASS |
| Unsigned assembly rejected by default, admitted on opt-in | PASS |
| P/Invoke refused by default, first import named | PASS |
| Framework-only image refused on CoreCLR host, admitted on Mono | PASS |
| Manifest `clr_disable_task_scheduler` drives profile flag | PASS |
| Deterministic topological load plan, upgrade recorded | PASS |
| Downgrade / missing / untrusted-token each fail distinctly | PASS |
| Dependency cycle reported with members | PASS |
| Tick boundary ordering on virtual clock | PASS |
| Every truncation prefix errors, never panics | PASS |
| Compressed-uint encoding matches ECMA-335 vectors | PASS |
| Token derivation matches independent SHA-1 vector | PASS |

## Authorization gates (in order)

1. Assembly identity present, else `NotResource`.
2. BCL target accepted by host flavor, else `BclMismatch`.
3. References `CitizenFX.Core`, else `NotResource`.
4. Declares a `BaseScript`-derived type, else `NotResource`.
5. Signed (or `allow_unsigned_resources`), else `Unsigned`.
6. No unmanaged imports (or `allow_native_calls`), else `NativeCall`.

## Profile semantics

| Property | mono | coreclr |
|----------|------|---------|
| Accepts NetFramework BCL | yes | no |
| Accepts Mono BCL | yes | no |
| Accepts CoreClr BCL | no | yes |
| Task scheduler | on unless manifest disables | on unless manifest disables |
| Execution | BLOCKED_EXTERNAL | BLOCKED_EXTERNAL |
