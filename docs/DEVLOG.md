# DEVLOG

## 2026-09-15 — Phase 0 + Phases 1-8 foundational crates

- Created Cargo workspace with 9 crates: ald-core, ald-config, ald-protocol,
  ald-device-identity, ald-identity, ald-events, ald-rpc, ald-resource, ald-scheduler.
- Each crate ships unit tests (parse/roundtrip/validation/leak-detection/runaway).
- Implemented: config validation, protocol header+channels+flags, event codec,
  keyed device fingerprint, identity aggregation + ban-confidence, namespaced event bus
  with protected namespaces, RPC registry with timeouts, resource manifest/lifecycle/
  capabilities/dep-resolution, scheduler workload classes + per-resource budgets +
  runaway detector.
- Docs written: ARCHITECTURE, ROADMAP, IMPLEMENTATION_STATUS, PROTOCOL, SECURITY_MODEL,
  PRIVACY, DEVLOG, security/THREAT_MODEL.
- Build blocker discovered: MSVC `link.exe` absent on host; `/usr/bin/link` shadows it.
  Interim `.cargo/config.toml` routes MSVC target through bundled `rust-lld`. This alone
  still needs MSVC import libs (kernel32.lib etc.), so installing Visual Studio 2022
  Build Tools + Windows 11 SDK via winget. First `cargo test` runs after install completes.
- Next: compile + run all crate tests, then proceed to Phase 4 transport (ald-network),
  Phase 9/10 script runtimes, Phase 11 server runtime.
