# ALDIVINE

**Next-generation GTA V multiplayer platform** — Rust-native, server-authoritative, and fully independent of FiveM.

Aldivine is a complete multiplayer ecosystem: launcher, client runtime, server
runtime, networking stack, gameplay framework, administration platform, and
developer tooling. It is designed so that legitimate GTA V owners can run and
play on Aldivine servers without any third-party multiplayer bridge.

---

## Why another multiplayer platform

Existing GTA V multiplayer ecosystems carry a decade of architecture debt:
single-threaded servers, script runtimes with unrestricted host access,
framework state in global mutable tables, and admin tooling bolted on after
the fact. Aldivine starts from a different set of assumptions:

- **Server authority is the default, not an afterthought.** Money, inventory,
  and entity state live in validated services. A client never tells the server
  what it has; it asks what it can do.
- **Resources are sandboxed.** A script gets only the capabilities its manifest
  declares — filesystem, network, database, and player-management access are
  all opt-in and administrator-approved.
- **The scheduler is a real component.** Every resource gets a CPU budget, a
  memory budget, an event rate limit, and a network event limit. Runaway
  resources are detected and throttled, not just logged.
- **Compatibility is a migration path, not the foundation.** Existing ESX,
  QBCore, and Qbox resources can run through adapters, but native Aldivine
  resources always get the full feature set.

## The ecosystem

| Component | What it is |
|---|---|
| **NovaGate Launcher** | Tauri/Rust/React launcher — game detection, server browser, update channels, signed manifests |
| **Astryn Client** | Native client runtime — AstraNet, resource lifecycle, Lua + JS runtimes, entity replication |
| **Aldivine Server Runtime** | Headless cross-platform server (Linux first-class) — config validation, console, metrics, graceful shutdown |
| **Aldivine Framework** | Native gameplay framework — identity, players, economy, inventory, jobs, permissions |
| **Aegis Control** | Axum + React administration platform — dashboard, player management, profiler, network inspector, audit log |
| **AstraNet** | Hybrid networking — reliable control channel plus low-latency datagrams, with per-channel rate limiting and an event firewall |
| **ACL** | Aldivine Compatibility Layer — clean-room adapters for ESX, QBCore, and Qbox |

## Architecture

```
NovaGate Launcher
      |
Astryn Client  ──────────────►  Aegis Control
      |                               |
      | AstraNet                      | Axum + React
      v                               v
Aldivine Server Runtime  ◄────  administration,
      |                          telemetry, audit
      |
      ├── AstraNet (transport, reliability, firewall)
      ├── Aldivine Scheduler (budgets, lanes, runaway detection)
      ├── Aldivine Framework (economy, inventory, jobs, players)
      └── Game resources (Lua / JavaScript / TypeScript)
```

Everything above the game runs on Rust. The only place GPU compute is used is
where workloads are genuinely massively parallel — batch transforms, spatial
queries, culling. Gameplay logic, networking, and scripting stay on CPU; that
is where they belong.

## Repository layout

```
crates/              Core libraries
  ald-core/          Error types, IDs, time helpers
  ald-config/        Typed server + resource configuration
  ald-protocol/      Packet framing, channels, event codec
  ald-network/       AstraNet: transport, reliability, rate limiting, firewall
  ald-ecs/           Entity model, spatial partition, interest management
  ald-replication/   Baseline + delta state replication
  ald-resource/      Manifest, lifecycle, capabilities, dependency resolver
  ald-scheduler/     Workload lanes, budgets, runaway detection
  ald-script-lua/    Lua 5.4 runtime (mlua), sandboxed
  ald-script-js/     QuickJS runtime (rquickjs), sandboxed
  ald-security/      Event firewall, payload validation
  ald-permissions/   Platform / server / framework / resource RBAC
  ald-events/        Namespaced event bus with protected namespaces
  ald-rpc/           Typed request-response with timeouts
  ald-database/      Pooling, transactions, migrations, slow-query logging
  ald-telemetry/     Metrics, structured logging
  ald-audit/         Immutable-style audit events
  ald-identity/      Player identity aggregation and ban confidence
  ald-compat/        ACL trait + coverage types
  acl-esx/           ESX adapter
  acl-qbcore/        QBCore adapter
  acl-qbox/          Qbox adapter

client/astryn/       Astryn client runtime
server/ald-server/   Aldivine Server Runtime
framework/aldivine/  Aldivine Framework
cli/ald/             Developer CLI
docs/                Architecture, protocol, security model, threat model
```

## Getting started

Requirements: a stable Rust toolchain and, on Windows, the MSVC build tools.

```sh
# Build everything
cargo build --release

# Run the full test suite
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

### Developer CLI

```sh
# Scaffold a new resource (Lua is the default; --js and --ts also work)
ald new my-job ./my-job --lua

# Validate a manifest or a server config
ald resource validate my-job/ald_manifest.toml
ald config validate server.toml

# Get a migration report for an existing FiveM resource.
# Exits 2 when it finds unsupported calls, so CI can gate on it.
ald migrate ./legacy-resource
```

The migration report lists every ESX / QBCore / Qbox / FiveM call it finds,
classifies each as supported / partial / unsupported / FiveM-only, detects the
framework flavor, and estimates migration difficulty. It does not rewrite code
— confidence is too low for that to be safe.

### Writing a resource

A resource is a directory with an `ald_manifest.toml`:

```toml
name = "my-job"
version = "0.1.0"
server_scripts = ["server/init.lua"]
client_scripts = ["client/init.lua"]

# Capabilities the resource needs. Nothing is granted by default.
capabilities = ["database", "player.manage"]
```

```lua
-- server/init.lua
Aldivine.Events.on('my-job:server:start', function(payload)
    print('my-job server started')
end)
```

Resources run in isolated contexts. `io`, `os`, `package`, `debug`, `require`,
and `loadfile` are removed from the Lua environment; the JavaScript runtime is
equivalently locked down. A resource that needs to read a file asks for the
`filesystem.read` capability in its manifest.

## Economy, inventory, and the permission model

The framework services exist because the alternative — global mutable tables
that any script can write to — is how money gets duplicated in production.

- **Economy.** All balances in cents. Transfers validate, debit, credit, and
  write a ledger entry with no await point in between, so a transfer is never
  observable half-applied. Money supply is conserved and testable.
- **Inventory.** Items are registered server-side and granted server-side.
  Weight limits, stack limits, and container nesting depth are enforced. A
  transfer that fails at the destination rolls the source back — no dupes, no
  losses.
- **Permissions.** Four layers: platform, server, framework, resource.
  Granular capabilities like `players.kick`, `resources.restart`, and
  `console.execute` are checked at the platform layer, never left to framework
  goodwill.

## Networking

AstraNet separates control from realtime state:

- **Control / auth / config / resource metadata** — reliable, encrypted.
- **Realtime entity state** — low-latency datagrams.

Logical channels: `CONTROL`, `EVENTS_RELIABLE`, `EVENTS_UNRELIABLE`,
`ENTITY_STATE`, `VOICE_METADATA`, `RESOURCE_TRANSFER`, `ADMIN`, `HEARTBEAT`.

Packets carry protocol version, session id, channel, sequence, tick, flags,
and payload size. Reliability is per-channel: the receive window detects
duplicates and replays, ordering is applied where it matters, and unordered
datagrams are left alone. MTU awareness prevents fragmentation; tiny realtime
packets are deliberately not compressed.

The **event firewall** rejects malformed payloads before gameplay code ever
sees them. Each network event declares its direction, schema, rate limit,
required permission, max payload, and allowed resource. Anti-spam and
anti-flood are enforced at the protocol layer, not in script handlers.

Replication is interest-managed: nearby high-priority entities update at 20 Hz,
distant ones at 2 Hz, irrelevant ones not at all. Baselines and deltas keep
bandwidth flat as entity counts grow.

## Compatibility

Aldivine does not claim that existing FiveM resources work. It claims, per API,
which ones do — and the claim is backed by tests, not estimation.

Coverage for `ESX.GetPlayerFromId`, `QBCore.Functions.GetPlayer`, and their
peers is `SUPPORTED` — routed through the authoritative framework services.
Calls that would require a resource to create money or items server-side are
`PARTIAL`, permitted only from server context. Calls with no safe
server-authoritative equivalent — `Citizen.CreateThread`, blocking waits,
legacy callback registries — are `NOT_SUPPORTED`, and `ald migrate` tells you
exactly which ones your resource uses.

The full matrix lives in [docs/compatibility/FIVEM_API_MATRIX.md](docs/compatibility/FIVEM_API_MATRIX.md).

## Security posture

Threat model and security model are written down, not implicit:

- [docs/security/THREAT_MODEL.md](docs/security/THREAT_MODEL.md) — malicious
  clients, malicious resources, event flooding, packet spoofing, resource
  tampering, admin compromise, credential theft, injection, DoS, sandbox
  escape.
- [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md) — authentication, session
  tokens, replay protection, schema validation, rate limiting, event ACL,
  audit logging.

Secrets are never committed. `.env.example` documents the shape; real
deployments use server configuration. Secrets are redacted from logs and crash
reports.

## Legal

Aldivine assumes users legitimately own GTA V. It does not implement DRM
bypass, license cracking, entitlement circumvention, anti-cheat bypass, or
support for pirated copies. It does not permanently modify a user's GTA
installation — all Aldivine files live under `%LOCALAPPDATA%/Aldivine/` on
Windows. Where game-runtime integration would require unavailable proprietary
interfaces, Aldivine defines an abstraction and documents the external
requirement rather than inventing behavior.

FiveM compatibility is clean-room: adapters implement observed API *semantics*
from documented public behavior. No Cfx.re source is used.

## Project status

Aldivine is under active development. Current state is tracked honestly, per
subsystem, in [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md)
using `IMPLEMENTED`, `PARTIAL`, `BLOCKED_EXTERNAL`, and `PLANNED`. Nothing is
marked complete while it is still a stub.

Core runtime, networking, scheduler, script runtimes, server runtime,
framework services, and compatibility adapters are implemented and tested.
NovaGate and Aegis are in progress.

## License

All rights reserved. See the LICENSE file.
