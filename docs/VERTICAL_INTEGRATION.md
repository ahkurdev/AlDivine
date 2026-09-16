# Vertical Integration

Live state of the user journey:

NovaGate -> Astryn -> AstraNet -> Server -> Resources -> Framework -> World

## Proven by test (2026-09-16)

`server/ald-server/tests/vertical_e2e.rs` runs the real path on loopback:

1. `Server::start` binds AstraNet UDP, discovers `base-resources/`, walks the
   13-state lifecycle (VALIDATING -> DEPENDENCY_RESOLUTION ->
   SCRIPT_HOST_STARTING -> MIGRATIONS_READY -> HEALTH_CHECKING -> HEALTHY),
   publishes the SHA-256 resource manifest into admission, and serves the
   Aegis API on `127.0.0.1:40120`.
2. `AstrynClient::start` walks the real handshake: `ClientHello` ->
   `ServerHello` + `AuthChallenge` -> `ClientAuth` (nonce echo) -> `AuthResult`
   + `IdentityRequest` -> `IdentityResponse` (device id) -> `DeferralDone` ->
   `EntitlementRequest` -> `EntitlementResult` -> `ClientReady` ->
   `QueueAdmitted` + `ManifestOffer` + `ServerReady { world_join: true }`.
3. The client reaches `Running` only on `ServerReady`. No server on the
   address means `Failed`, never `Running` (`no_server_never_reaches_running`).
4. On `ServerReady` the server registers the player in
   `aldivine-framework` `PlayerService` (asserted `online_count >= 1`) and
   pushes a `ald:server:welcome` event on EventReliable, which the client
   captures (asserted name + payload).
5. The client downloads `spawn/ald_manifest.toml` over the ResourceTransfer
   channel in 32 KiB chunks, reassembles via `ald-download`, verifies SHA-256
   against the manifest, and commits to `ContentCache` (bytes asserted equal,
   cache verify asserted true).

## Admission pipeline (all wired)

- Protocol: `ald-protocol::handshake` (`HandshakeMessage`, `HandshakeState`).
- Server dispatch: `ServerState::network_loop` routes by `Channel` behind an
  ingress gate: per-peer token bucket (300 burst, 150/s), client channel
  allowlist, unknown peers limited to `Auth`, rejected sessions dropped.
  `Auth` payloads go to `AdmissionManager`; bad payloads are counted and
  rejected, never panic.
- Identity: `check_identity_policy` against server `[identity]` config.
  Device id is required by default; the client sends an ephemeral id marked
  confidence `low`. Keyed HMAC device attestation stays PARTIAL.
- Entitlement: free-by-default `EntitlementResult`; package grants PARTIAL.
- Deferrals: `DeferralSession` with a ban-check gate; waiting/deny paths send
  `DeferralUpdate`/`Reject`, never hang (watchdog `ALD-QUEUE-TIMEOUT`).
- Queue: real `ald-queue` join; `Queued { position }` or `Admitted`.
- Manifest: `ManifestOffer` carries name + SHA-256 + size of each discovered
  `ald_manifest.toml`. Chunk transport over AstraNet: WIRED (v1, same socket).

## Resource execution (real, Lua slice)

`LifecycleSupervisor::transition_start` walks explicit lifecycle states:
VALIDATING (manifest parse) -> DEPENDENCY_RESOLUTION (sibling check) ->
SCRIPT_HOST_STARTING (sandboxed `LuaRuntime` with `Aldivine.Events`,
executes every `server_scripts` entry) -> MIGRATIONS_READY (fails honestly
when migrations are declared but no runner is wired) -> HEALTH_CHECKING ->
HEALTHY. Any failure marks `Failed` with the real cause. A resource is
"serving" only in HEALTHY (or DEGRADED); `spawn` loads for real.
The Lua host is a declared native boundary: `ald-server` enables it only via
`--features lua` (default closure stays C-free, `audit-native` green);
without the feature, start fails honestly instead of pretending.

## Download transport (real, v1)

`ald-protocol::transfer`: JSON request, JSON-header + raw-bytes response,
32 KiB cap, offset/len validation fail-closed. `ald-server::transfer`
serves files under the resource root with canonical-path containment
(traversal rejected). `AstrynClient::fetch_resource_file` drives the
missing-range loop over `ald-download`, enforces a stall bound, verifies
SHA-256, commits to `ald-cache`. CDN mirrors / range resume across
restarts stay PLANNED.

## Aegis API (real, local)

`ald-server` serves Axum on `127.0.0.1:40120` (non-loopback binds refused):

- `GET /health`, `GET /status`, `GET /resources`
- `POST /resources/:name/:action` (start/stop/restart) drives the real
  lifecycle channel.
- `GET /setup/status` (first-run detection), `POST /setup/complete`
  (validates and writes `server.cfg`).

React UI: `aegis/web` (dashboard + first-run setup, sky theme); built by CI.

## Honest blockers

- GTA present on this box: Epic install detected earlier at
  `C:\\Program Files\\Epic Games\\GTAV`. Game Bridge / Native Bridge / NUI /
  F8 / asset runtime stay BLOCKED_EXTERNAL until exercised against the game.
- Asset formats: recognition PASS (synthetic); content validation, mount
  into the game, and rendering BLOCKED_EXTERNAL.
- DB: capability/conformance suites green; live Postgres/MySQL/MariaDB
  conformance BLOCKED_EXTERNAL (no DB server here).
- Node/CLR: resolution/authorization halves only; execution BLOCKED_EXTERNAL.
- Voice audio transport: BLOCKED_EXTERNAL.
