# IMPLEMENTATION STATUS

Live status of every major subsystem. Updated each phase.

Legend: IMPLEMENTED | PARTIAL | BLOCKED_EXTERNAL | PLANNED | EXPERIMENTAL

| Subsystem | Crate / Dir | Status | Notes |
|-----------|-------------|--------|-------|
| Core errors/ids/time | ald-core | IMPLEMENTED | error enum (+Network), AldivinePlayerId (ULID), EntityId (generation), time |
| Config + validation | ald-config | IMPLEMENTED | server/network/identity/security sections, strict validate() |
| Protocol framing | ald-protocol | IMPLEMENTED | 32-byte header, 9 channels, flags, encode/decode, event codec |
| Citizen serialization | crates/ald-serialization-compat | IMPLEMENTED | hand-written msgpack encoder/decoder matching CitizenFX wire: ext 20/21/22/23 (vec2/3/4, quat) big-endian f32, ext 10/11 funcref preserved as opaque, nil vs false distinct; byte-exact golden tests against reference msgpack output; 15 tests |
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
|| NovaGate | apps/novagate (novagate-core) | IMPLEMENTED | GTA V install detection (registry/Steam-library/Epic-manifest, decisive 3-file structure check), distribution classification, detection-vs-entitlement separation enforced by type, AldivinePaths layout (%LOCALAPPDATA%/Aldivine, never touches game install), signed update manifests (signature/channel/min-version/downgrade rejection, payload hash verification), server browser state (favorites, bounded dedup history, direct-connect validation); 31 tests |
|| Aegis | aegis/backend (aegis-core) | IMPLEMENTED | PBKDF2-HMAC-SHA256 password hashing (constant-time verify, random salt, never plaintext), RFC 6238 TOTP with skew window (HOTP core verified against RFC 4226 vectors 755224/287082), session store (absolute+idle TTL, immediate invalidation, GC), append-only audit log, brute-force lockout (per-account, audited), permission-gated warn/kick/ban with denied-action auditing, resource profiler (P50/P95/P99 nearest-rank, slow detection, bounded window), alerts (dedup, resolve, reconcile, webhooks), incident timeline correlation; 82 tests |
| Time synchronization | crates/ald-timesync | IMPLEMENTED | NTP-style offset/RTT estimator keeping lowest-RTT samples, jitter spread, clamped interpolation delay, server-authoritative monotonic + wall clock pair, client time-claim bounds rejecting stale/forged history (lag compensation never trusts client timestamps); 10 tests |
| Feature negotiation + deprecation UX | crates/ald-negotiation | IMPLEMENTED | server-authoritative Hello/feature negotiation with typed outcomes (Selected/ClientOnlyUnsupported/RequiresClientUpdate/RequiresServerUpdate/DisabledByPolicy), structured protocol deprecation (ClientTooOld/ServerTooOld/Incompatible) carrying UpgradeAction with channel + min version, strict required-feature hard-fail, capability intersection; 11 tests |
| Server query protocol | crates/ald-query | IMPLEMENTED | anonymous UDP probe ("ALDQ" magic, strict length/magic/version validation), bounded native JSON response with progressive shedding (players -> icon -> extra), unlisted servers answer counts only, per-source rate limiter with GC (anti-amplification), legacy /info.json /players.json /dynamic.json derived from one ServerStatus; 13 tests |
| fxmanifest / manifest normalization | crates/ald-fxmanifest | IMPLEMENTED | hand-written Lua table-literal lexer + parser for fxmanifest.lua / __resource.lua (no Lua interpreter dependency), NormalizedManifest target, all documented directives parsed (fx_version, game(s), client/server/shared scripts, files, ui_page, exports, server_exports, dependency/provide, data_file, level meta, loadscreen, server_only, lua54, node_version, clr_disable_task_scheduler, use_experimental_fxv2_oal, convar_category), unknown directives classified UNSUPPORTED and preserved verbatim (never silently discarded), fileserver_add TRANSLATED to streaming/CDN origin, convar_category parsed into Aegis form-schema (convar/name/help/default/min/max/type), ald_manifest.toml also supported; 20 tests |
| CfxLua compatibility frontend | crates/ald-cfxlua-compat | IMPLEMENTED | vector2/vector3/vector4/quaternion value types with Cfx field semantics + arithmetic metatables (__add/__sub/__mul vec*vec and vec*scalar, __tostring, __eq), joaat one-at-a-time hashing exposed as joaat() global and as a backtick source preprocessor (`adder` -> joaat("adder")) with reference values verified independently, json global with Cfx array-vs-object table detection, msgpack global with a self-contained ext-free msgpack codec (golden wire bytes for fixint/nil/bool/fixstr/empty arr+map, truncated-input rejected), promise global + Citizen.Await with synchronous resolve and reject-as-error; 20 tests |
| Aldivine VFS (virtual filesystem) | crates/ald-vfs | IMPLEMENTED | resource-scoped sandbox mount table with @resource/path resolution, cross-resource access denied unless explicitly shared, one-way share(), lexical traversal bound (net depth must never go negative), Windows reserved device-name rejection (COM1-9/LPT1-9, con/prn/aux/nul), alternate-data-stream rejection, NUL byte / RTLO / combining-mark rejection, write path uses temp-file + canonical re-verify + atomic rename, symlink escape blocked by canonical containment on every read and write; 16 tests incl. a 4600-case adversarial path fuzz sweep proving every generated path either errors or resolves inside the sandbox root |
| Aldivine Edge gateway | crates/ald-edge | IMPLEMENTED | pre-auth admission (per-IP + origin connection caps, banned IP/CIDR), query rate limiting isolated from gameplay budget, origin shielding (fail_closed + shield_allow_networks), HMAC-SHA256 authenticated real-IP forwarding with freshness window and constant-time compare, stdlib-only SHA-256 verified against FIPS 180-2 vectors and HMAC against RFC 4231 TC2; 15 tests |

## Build Environment

- Host: Windows 11, Rust stable 1.98.1 (x86_64-pc-windows-msvc).
- Visual Studio 2022 Build Tools + Windows 11 SDK (10.0.22621.0) installed.
  Build env (LIB/INCLUDE/PATH) is exported before cargo in this session.
- Full workspace `cargo build` and `cargo test` pass green (~100 unit tests).
- `cargo clippy` passes with only formatting warnings (rustfmt diffs).
