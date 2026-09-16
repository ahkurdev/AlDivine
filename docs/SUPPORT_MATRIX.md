# Aldivine Certified Support Matrix

This document defines the certified compatibility matrix for Project Aldivine releases, governed by `ald-compat-lab` and the Single Source of Truth specification.

## Compatibility Honesty Rule

"100% Supported" strictly means **100% PASS on all gates of a defined certified matrix**.
If a single gate fails or is marked skipped / external blocker, Aldivine does NOT claim 100% universal compatibility.

---

## Matrix: Aldivine v0.1.0-RC1 Certified Matrix (`matrix-2026-v1`)

## Evidence tiers

- SYNTHETIC PASS: unit/integration test against doubles or synthetic data.
- INTEGRATION PASS: real components wired together (loopback, no game).
- REAL RUNTIME PASS: exercised against the live target (GTA, real DB).
- BLOCKED_EXTERNAL: needs a target unavailable on this box. Never counted as PASS.

| Subsystem / Gate | Target / Build | Certified Status | Notes |
|---|---|---|---|
| **Host Build** | Windows 11 x86_64 | **PASS** | Toolchain pinned to 1.98.1 in `rust-toolchain.toml` |
| **Native Dependency Audit** | `ald-server` | **PASS** | 0 unexpected C/C++ dependencies in core closure |
| **Vertical Join (non-GTA)** | Astryn -> Server loopback | **INTEGRATION PASS** | `vertical_e2e`: Hello/Auth/Identity/Deferral/Queue/Manifest/Ready, client Running, framework online >= 1, welcome event delivered, chunked download verified |
| **Aegis HTTP API** | 127.0.0.1:40120 | **INTEGRATION PASS** | health/status/resources + real lifecycle actions + first-run setup/status + setup/complete provisioning |
| **Aegis React UI** | aegis/web dist | **INTEGRATION PASS** | `npm run build` green (tsc + vite): setup wizard + live dashboard driving the real API |
| **Chunk Download Transport** | ResourceTransfer loopback | **INTEGRATION PASS** | 32 KiB chunks, resume ranges, SHA-256 verify, cache commit; CDN mirrors PLANNED |
| **GTA Legacy Support** | Build 2699 | **SYNTHETIC PASS** | Build definition registered; real Game Bridge BLOCKED_EXTERNAL |
| **GTA Enhanced Support** | Build 3095 | **SYNTHETIC PASS** | Build definition registered; real Game Bridge BLOCKED_EXTERNAL |
| **CfxLua Profile** | Lua 5.4 Compat | **PASS** | Vector math, joaat, JSON, msgpack, promise/await verified |
| **Native Client JS** | QuickJS Embed | **PASS** | Sandboxed Citizen client surface verified |
| **Node.js Compatibility** | Node 16 / 22 | **PARTIAL** | Engine-independent resolution pass; native V8 host isolated |
| **CfxCLR / .NET Compatibility**| Mono / CoreCLR | **PARTIAL** | ECMA-335 metadata reader pass; CIL runtime host isolated |
| **FXManifest Parser** | v1 / v2 / ald_manifest | **PASS** | All documented directives parsed into NormalizedManifest |
| **Citizen Event Surface** | Reliable & Unreliable | **PASS** | RegisterNetEvent, AddEventHandler, TriggerEvent pass |
| **State Bags** | Strict Ownership | **PASS** | GlobalState, Entity, Player state bags verified |
| **Routing Buckets** | Dimensions 1:1 | **PASS** | Dimension isolation and bucket lockdowns pass |
| **Asset Formats** | .ytd, .yft, .ydd, .ydr | **SYNTHETIC PASS (recognition)** | Extension/stream-class recognition; content validation + game mount BLOCKED_EXTERNAL |
| **DataFile Registry** | vehicles, handling, etc. | **SYNTHETIC PASS (recognition)** | Vehicle/handling/carcols graph seed; game registration BLOCKED_EXTERNAL |
| **Database: PostgreSQL** | PostgreSQL 14..16 | **SYNTHETIC PASS** | Capability/conformance doubles green; live server conformance BLOCKED_EXTERNAL |
| **Database: MySQL** | MySQL 8.0+ | **SYNTHETIC PASS** | Capability/conformance doubles green; live server conformance BLOCKED_EXTERNAL |
| **Database: MariaDB** | MariaDB 10.6+ | **SYNTHETIC PASS** | Capability/conformance doubles green; live server conformance BLOCKED_EXTERNAL |
| **ACL: ESX Adapter** | v1.Final / Legacy | **PASS** | Read-only projection of xPlayer, jobs, accounts |
| **ACL: QBCore Adapter** | QBCore standard | **PASS** | Read-only projection of Player, items, money |
| **ACL: Qbox Adapter** | Qbox standard | **PASS** | Read-only projection of Player state and exports |
| **Streaming Pipeline** | Chunked & Resumable | **PASS** | DRR fair scheduler, watchdog, content cache repair |
| **Security & Anti-Cheat** | Server-Authoritative | **PASS** | Bounds check, speed verification, SSRF blocking, DDoS buckets |
| **Safe Mode & Symbols** | Crash Loop Protection | **PASS** | Minidump parsing, fingerprinting, auto-quarantine |
| **Diagnostic Replay** | Deterministic Debugger | **PASS** | Tick-accurate scrubbing, breakpoints, entity replay |
| **Aegis Node Supervisor**| Process Ownership | **PASS** | Watchdog, crash loops, build rollback, multi-instance |
| **Distributed Sharding** | Player Handoff | **PASS** | Two-phase handoffs, epoch ownership leases |
