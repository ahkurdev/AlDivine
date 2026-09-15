# IMPLEMENTATION STATUS

Live status of every major subsystem. Updated each phase.

Legend: IMPLEMENTED | PARTIAL | BLOCKED_EXTERNAL | PLANNED | EXPERIMENTAL

| Subsystem | Crate / Dir | Status | Notes |
|-----------|-------------|--------|-------|
| Core errors/ids/time | ald-core | IMPLEMENTED | error enum (+Network), AldivinePlayerId (ULID), EntityId (generation), time |
| Config + validation | ald-config | IMPLEMENTED | server/network/identity/security sections, strict validate() |
| Protocol framing | ald-protocol | IMPLEMENTED | 32-byte header, 9 channels, flags, encode/decode, event codec |
| Device identity | ald-device-identity | IMPLEMENTED | keyed HMAC fingerprint, confidence, versioning |
| Identity service | ald-identity | IMPLEMENTED | PlayerIdentity, Steam hex (correct formula), providers, ban match confidence |
| Event system | ald-events | IMPLEMENTED | namespaced, priority, protected namespaces, Arc handlers, no-deadlock dispatch |
| RPC | ald-rpc | IMPLEMENTED | request IDs, timeouts, pending tracking, expire() |
| Resource system | ald-resource | IMPLEMENTED | manifest parse, lifecycle+leak detect, capabilities, dep resolution |
| Permissions (RBAC) | ald-permissions | IMPLEMENTED | roles + wildcard + granular per-principal grants |
| AstraNet transport | ald-network | IMPLEMENTED | UDP transport, session manager + idle expiry, reliability (replay/dup/order, 32-bit wrap), token-bucket rate limiter, event firewall (direction/size/permission/channel ACL), IP normalize + trusted-proxy CIDR, fragmentation |
| Scheduler | ald-scheduler | IMPLEMENTED | workload classes, budgets, runaway detector, async executor lanes (ordered/parallel/background) with live stats |
| ECS entities | ald-ecs | IMPLEMENTED | slot-based EntityId generation, spawn/despawn, component map |
| Security utils | ald-security | IMPLEMENTED | secret redaction, log sanitization |
| Telemetry | ald-telemetry | IMPLEMENTED | metrics counters/histograms |
| Audit | ald-audit | IMPLEMENTED | structured audit events |
|| Database layer | ald-database | IMPLEMENTED | repository trait, pool config, query timing, slow-query detection, RAII transactions with rollback-on-drop, canonical money-transfer tx, deterministic migration runner with drift detection |
|| Entitlement service | crates/ald-entitlement | IMPLEMENTED | entitlement models (FREE/PAID/SUBSCRIPTION/PRIVATE/DEVELOPER/BETA/INVITE_ONLY), server registration with signed credentials, package entitlement resolution; 13 tests |
|| Package format + signing | crates/ald-package | IMPLEMENTED | .alpkg manifest, Ed25519 signing over canonical bytes, SHA-256 payload verification, OPEN/SIGNED/PROTECTED/PRIVATE policies, tamper + wrong-key + stale-hash rejection; 10 tests |
|| Login pipeline | crates/ald-identity/src/pipeline.rs | IMPLEMENTED | identity requirement policy, structured ALD-* connection codes, Epic-owner-not-blocked rule, trusted-proxy IP resolution; 13 tests |
|| Compat abstraction | ald-compat | PARTIAL | ACL trait; ESX/QBCore/Qbox adapters PLANNED |
| Lua runtime | crates/ald-script-lua | IMPLEMENTED | mlua 0.10 Lua 5.4 vendored; sandbox removes io/os/package/debug/jit/require/loadfile; 5 tests |
| JS runtime | crates/ald-script-js | IMPLEMENTED | rquickjs 0.13 QuickJS bindgen; single shared context; 3 tests |
| Server runtime | server/ald-server | IMPLEMENTED | config-validated startup, AstraNet listener, LifecycleSupervisor (single-writer start/stop/restart via channel, leak detection), console with audited commands, telemetry loop, graceful shutdown with SIGTERM/Ctrl-C; 6 tests |
| Replication | crates/ald-replication | IMPLEMENTED | baseline+delta, tier-driven send rate, coalescing, queue cap/drop with stats; 6 tests |
| Spatial partition + interest | crates/ald-ecs/src/spatial.rs | IMPLEMENTED | uniform grid, radius query, interest tiers (20/10/2 Hz), replication set excludes self + far; 8 tests |
| Astryn client | client/astryn | IMPLEMENTED | lifecycle state machine, background connect task, bounded graceful shutdown; failures degrade to Failed not panic; 2 tests |
| Framework | framework/aldivine | IMPLEMENTED | economy (atomic transfer + ledger, money-supply conservation), inventory (weight/stack/nesting limits, rollback on failed transfer), jobs (grades, duty, salary, rank), players (characters, slot limit, unique names); 26 tests |
| Compatibility (ESX/QB/Qbox) | crates/acl-esx, acl-qbcore, acl-qbox | PARTIAL | LegacyAdapter trait; ESX 4 tests, QBCore 3, Qbox 2; read-only player projections over framework services, honest per-API coverage matrix in docs/compatibility/FIVEM_API_MATRIX.md; mutations server-context only; event/callback/export/command/state-bag adapters still PLANNED |
| NovaGate | apps/novagate | PLANNED | Tauri |
| Aegis | aegis/ | PLANNED | Axum + React |

## Build Environment

- Host: Windows 11, Rust stable 1.98.1 (x86_64-pc-windows-msvc).
- Visual Studio 2022 Build Tools + Windows 11 SDK (10.0.22621.0) installed.
  Build env (LIB/INCLUDE/PATH) is exported before cargo in this session.
- Full workspace `cargo build` and `cargo test` pass green (~100 unit tests).
- `cargo clippy` passes with only formatting warnings (rustfmt diffs).
