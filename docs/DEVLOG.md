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

## 2026-09-15 — Master spec alignment + PHASE 5 + PHASE 6

- Adopted the ULTIMATE MASTER SPECIFICATION as single source of truth; ROADMAP
  rewritten to the 74-phase model with honest per-phase statuses.
- Baseline verified before new work: full workspace `cargo build` green and
  336 tests / 0 failures across 68 test binaries.
- Fixed a real environment bug: the session PATH leaked an incompatible
  MSVC layout (Program Files/ x86 absent, cl.exe exit 0xc0000017). Builds now
  run through `.env-build.sh`, which sources the installed
  14.44.35207 toolchain + Windows 11 SDK 10.0.22621.0 explicitly.
- PHASE 5 (time sync / negotiation / query): ald-timesync (NTP-style offset +
  RTT + jitter, clamped interpolation delay, server-authoritative clocks,
  client time-claim bounds), ald-negotiation (server-authoritative feature
  negotiation with typed outcomes and structured deprecation UX:
  ClientTooOld / ServerTooOld / Incompatible carrying UpgradeAction),
  ald-query (ALDQ probe protocol, bounded JSON with progressive shedding,
  unlisted servers answer counts only, anti-amplification rate limiter,
  legacy /info.json /players.json /dynamic.json from one ServerStatus).
- PHASE 6 (Aldivine Edge): pre-auth admission with per-IP and origin
  connection caps, banned IP/CIDR, query budget isolated from gameplay,
  origin shielding (fail_closed + shield_allow_networks), and HMAC-SHA256
  authenticated real-IP forwarding with freshness window + constant-time
  compare. SHA-256/HMAC are stdlib-only and verified against FIPS 180-2 and
  RFC 4231 TC2 vectors rather than asserted by inspection.
- 49 new tests (10 timesync, 11 negotiation, 13 query, 15 edge). All green.
- Next: PHASE 8 ald-vfs (@resource/path, sandbox writes, symlink/junction
  and reparse-point hardening, case-insensitivity, Unicode normalization,
  reserved Windows device names).

## PHASE 8 — Aldivine VFS (ald-vfs)

- Implemented resource-scoped virtual filesystem: mount table, @resource/path
  resolution, explicit one-way share() for cross-resource reads, sandboxed writes.
- Hardening, all backed by tests: lexical traversal bound (net depth < 0 rejected),
  Windows reserved device names (COM1-9 / LPT1-9 / con / prn / aux / nul),
  alternate data streams, NUL bytes, RTLO override and combining diacritics.
- Reads and writes re-canonicalize after following filesystem links, so a
  symlink or junction swapped in after validation still cannot escape.
- Writes go to a temp sibling then rename only after final containment check.
- 16 tests green, including a 4600-case adversarial path sweep that asserts
  every generated path either errors or resolves inside the sandbox root.
- Bugs found by the tests and fixed: ..  accepted as a resource name; COM10+
  falsely treated as a reserved device name (Windows reserves COM1-9 only);
  depth check ran on raw backslash separators before normalization;
  verify_within rejected the legit Windows absolute-prefix of a canonical root.
