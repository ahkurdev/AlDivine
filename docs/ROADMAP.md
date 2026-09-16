# ROADMAP

Dependency-ordered phase plan, aligned to the ULTIMATE MASTER SPECIFICATION
(single source of truth). Each phase compiles, tests, and documents before the
next begins. Allowed statuses: IMPLEMENTED, PARTIAL, EXPERIMENTAL,
BLOCKED_EXTERNAL, NOT_SUPPORTED, PLANNED.

| # | Phase | Status |
|---|-------|--------|
| 0 | Architecture, monorepo, docs, status model | IMPLEMENTED |
| 1 | Core Rust: errors, config, logging, tracing | IMPLEMENTED (ald-core, ald-config, ald-telemetry, ald-audit) |
| 2 | server.cfg + server.toml, NormalizedServerConfig, secrets, ACE/RBAC, resource commands | IMPLEMENTED (ald-servercfg 70 tests, ald-permissions, ald-security) |
| 3 | DB capability layer; Postgres/MySQL/MariaDB conformance | IMPLEMENTED (ald-database, ald-db-capability 15 tests) |
| 4 | AstraNet, protocol schema registry, Citizen serialization, crypto, sessions | IMPLEMENTED (ald-protocol, ald-network, ald-serialization-compat 15 tests, ald-package signing) |
| 5 | Time sync, feature negotiation, protocol deprecation UX, server query | IMPLEMENTED (ald-timesync, ald-negotiation, ald-query) |
| 6 | Aldivine Edge: DDoS ingress, pre-auth | IMPLEMENTED (ald-edge) |
| 7 | Scheduler, events, core event registry, RPC, backpressure, state foundation | PARTIAL (ald-scheduler, ald-events, ald-rpc, ald-replication done; core event registry + per-channel backpressure PLANNED) |
| 8 | VFS, symlink/junction hardening, sandbox | PLANNED (ald-vfs) |
| 9 | NormalizedManifest, fxmanifest.lua, __resource.lua, all documented directives | PLANNED (ald-fxmanifest) |
| 10 | Native Lua runtime, CfxLua compatibility | PARTIAL (ald-script-lua Lua 5.4 sandbox; CfxLua frontend PLANNED) |
| 11 | Client JavaScript runtime | IMPLEMENTED (ald-script-js, rquickjs) |
| 12 | Node 16 / Node 22 compatibility runtime, package.json, dependency install | PARTIAL (ald-script-node: profiles, manifest node_version selection, package.json parse, engines-node semver enforcement, Node resolution over NodeFileSource with node_modules walk + scoped/subpath, deterministic lock planning, npm integrity sha512/sha256 verify + content-address cache path, install-script-off / native-addon-blocked policy, 30-builtin capability table; JS evaluation BLOCKED_EXTERNAL — no engine vendored, see docs/NODE_COMPATIBILITY.md + docs/compatibility/NODE_MATRIX.md) |
| 13 | CfxCLR / .NET compatibility | PLANNED (ald-script-dotnet) |
| 14 | Native extension ABI | PLANNED (ald-native-extension) |
| 15 | Aldivine server runtime, server console/TUI | IMPLEMENTED (server/ald-server: startup, AstraNet listener, LifecycleSupervisor, console, graceful shutdown) |
| 16 | Connection admission, deferrals, queue, reserved slots | PLANNED (ald-queue, ald-deferrals) |
| 17 | Identity, entitlement, device ID, secrets, ban engine | IMPLEMENTED (ald-identity, ald-entitlement, ald-device-identity, login pipeline; aegis-core ban engine 82 tests) |
| 18 | Astryn bootstrap, Astryn client | PARTIAL (client/astryn lifecycle state machine; bootstrap + game bridge PLANNED) |
| 19 | GameEdition Legacy/Enhanced, GTA build manager, emergency compatibility | PLANNED (ald-game-builds, ald-game-editions) |
| 20 | Game Bridge | PLANNED (client/game-bridge) |
| 21 | Native bridge, entity/network handle facade | PLANNED (client/native-bridge, ald-natives) |
| 22 | Game event router, population manager, routing-bucket compat | PLANNED (client/game-events, ald-population) |
| 23 | Astryn developer console / F8 | PLANNED (client/developer-console) |
| 24 | Entity system, world sync, ownership, interest, dimensions, lag compensation | PARTIAL (ald-ecs + spatial grid + interest tiers + ald-replication; world sync/ownership/dimensions/lag comp PLANNED) |
| 25 | Persistent world, snapshot/journal, crash recovery | PLANNED (ald-persistence, ald-snapshot) |
| 26 | State bags, exact compatibility semantics | PLANNED (ald-state, ald-statebag-compat) |
| 27 | Asset format registry, comprehensive DataFile registry | PLANNED (ald-asset-registry, ald-datafiles) |
| 28 | Vehicle graph, clothing graph, MLO graph, audio/extra formats | PLANNED (ald-assets) |
| 29 | Download engine, fair scheduling, chunking, resume, watchdog, CDN auth/failover | PLANNED (ald-download, ald-streaming) |
| 30 | Content-addressed cache, atomic staging, repair, dedup | PLANNED (ald-cache) |
| 31 | Asset mounting, registration, residency, ready handshake | PLANNED (ald-mount, ald-residency) |
| 32 | FPS-aware streaming, memory/VRAM budgets, storage quotas | PLANNED |
| 33 | Embedded browser, NUI, secure origins, callbacks, WASM | PLANNED (ald-nui) |
| 34 | DUI runtime, runtime textures, world-space UI | PLANNED (ald-dui) |
| 35 | Input, key mapping | PLANNED (ald-input) |
| 36 | Client join/download/residency state machine | PLANNED |
| 37 | Aldivine framework, idempotent economy/inventory, migration contracts | IMPLEMENTED (framework/aldivine: economy, inventory, jobs, players; 26 tests) |
| 38 | Base resources | PLANNED (base-resources/) |
| 39 | ESX | PARTIAL (acl-esx, read-only projections, 4 tests) |
| 40 | QBCore | PARTIAL (acl-qbcore, read-only projections, 3 tests) |
| 41 | Qbox | PARTIAL (acl-qbox, read-only projections, 2 tests) |
| 42 | Common compatibility packs, voice/Mumble compat, profiles | PLANNED (ald-voice-compat) |
| 43 | NovaGate, installer, updater | IMPLEMENTED (apps/novagate: detection, signed updates, server browser; 31 tests) |
| 44 | Identity Cloud, Directory, Registry | PLANNED (services/) |
| 45 | Registry malware/package scanner | PLANNED (ald-package-scanner) |
| 46 | Astryn simulation client, deterministic scenarios | PLANNED (client/simulation-client) |
| 47 | Aegis Node Agent | PLANNED (agent/aegis-node) |
| 48 | Aegis first-run, sky blue UI, simple/advanced mode | PLANNED (apps/aegis-web) |
| 49 | Aegis framework/DB setup, existing server import, DB introspection | PLANNED |
| 50 | Aegis convar_category UI, provisioning engine | PLANNED |
| 51 | Aegis dashboard, console, resource manager, admin/RBAC, DB manager | PLANNED |
| 52 | Aegis assets/streaming, network, world, profiler, security | PARTIAL (aegis-core profiler/alerts/incidents; UI PLANNED) |
| 53 | Aegis recipes, transactional deployment, quarantine, maintenance/drain | PLANNED |
| 54 | Backups, DR, DB migration safety, aldivine.lock | PLANNED (ald-backup) |
| 55 | Packages, signing, entitlements, publisher protection | IMPLEMENTED (ald-package: .alpkg, Ed25519, tamper rejection; 10 tests) |
| 56 | Upgrade planner | PLANNED (ald-upgrade-planner) |
| 57 | Migration tooling, SDK, CLI, aldreport | PARTIAL (cli/ald + cli_e2e; aldreport PLANNED) |
| 58 | Anti-cheat, SSRF, DDoS, client integrity | PARTIAL (ald-edge pre-auth + ald-network firewall/ratelimit; anti-cheat + SSRF policy PLANNED) |
| 59 | Crash dumps, symbol service, crash fingerprinting, safe mode | PLANNED (ald-symbol-service) |
| 60 | Diagnostic replay, resource debugger | PLANNED (ald-replay) |
| 61 | Voice | PLANNED (ald-voice) |
| 62 | Windows artifact | PLANNED |
| 63 | Linux artifact | PLANNED |
| 64 | Artifact API, browser, channels, updater, rollback, SBOM | PARTIAL (NovaGate signed update manifests; artifact API PLANNED) |
| 65 | Central service HA, offline/fault behavior | PLANNED |
| 66 | Sharding groundwork, player handoff, distributed ownership contracts | PLANNED |
| 67 | Compatibility lab, synthetic + real OSS corpus, exact-version certification | PLANNED (compatibility/) |
| 68 | Hot reload stress, crash injection | PLANNED |
| 69 | Streaming chaos matrix, GPU/storage matrix | PLANNED |
| 70 | 24/48/72h soak tests | PLANNED (soak/) |
| 71 | Performance SLO calibration, hardware certification | PLANNED (benchmarks/) |
| 72 | Localization, accessibility, telemetry consent | PLANNED |
| 73 | Security review, OSS compliance, production validation | PLANNED |
| 74 | Release candidate, docs finalization, support matrix publication | PLANNED |

## Build environment

- Host: Windows 11, Rust stable 1.98.1 (x86_64-pc-windows-msvc).
- Visual Studio 2022 Build Tools (MSVC 14.44.35207) + Windows 11 SDK
  (10.0.22621.0). Build env (LIB/INCLUDE/PATH) exported via `.env-build.sh`.
- Full workspace `cargo build` and `cargo test` pass green.

## Next action

PHASE 10: ald-script-lua / ald-cfxlua-compat — Lua 5.4 runtime with the
CfxLua compatibility frontend: vector2/3/4 + quat, backtick joaat-style
compile-time hashes, json/promise/msgpack globals, Citizen.Await, module
loading, plus golden behavior tests. Depends on the rquickjs-style embed
already present in the workspace.
## Next action — DONE (PHASE 11)

PHASE 11 complete: ald-script-js Citizen-compatible client runtime, 16/16
tests green, workspace regression clean (see DEVLOG).

## Next action

PHASE 12 — Node 16 / Node 22 compatibility runtime (ald-script-node):
package.json, node_modules resolution, require(), timers, setImmediate,
setTick/clearTick, on/onNet/emit/emitNet, selected Node built-ins behind
sandbox/capability gates. QuickJS is NOT Node.js; do not pretend otherwise.
