# Vertical Integration

Live state of the user journey:

NovaGate -> Astryn -> AstraNet -> Server -> Resources -> Framework -> World

## Proven by test (2026-09-16)

`server/ald-server/tests/vertical_e2e.rs` runs the real path on loopback:

1. `Server::start` binds AstraNet UDP, discovers `base-resources/`, executes
   Lua server scripts into `Running`, publishes the SHA-256 resource manifest
   into admission, and serves the Aegis API on `127.0.0.1:40120`.
2. `AstrynClient::start` walks the real handshake: `ClientHello` ->
   `ServerHello` + `AuthChallenge` -> `ClientAuth` (nonce echo) -> `AuthResult`
   + `IdentityRequest` -> `IdentityResponse` (device id) -> `DeferralDone` ->
   `EntitlementRequest` -> `EntitlementResult` -> `ClientReady` ->
   `QueueAdmitted` + `ManifestOffer` + `ServerReady { world_join: true }`.
3. The client reaches `Running` only on `ServerReady`. No server on the
   address means `Failed`, never `Running` (`no_server_never_reaches_running`).
4. On `ServerReady` the server registers the player in
   `aldivine-framework` `PlayerService` (asserted `online_count >= 1`).

## Admission pipeline (all wired)

- Protocol: `ald-protocol::handshake` (`HandshakeMessage`, `HandshakeState`).
- Server dispatch: `ServerState::network_loop` routes by `Channel`; `Auth`
  payloads go to `AdmissionManager`; bad payloads are counted and rejected,
  never panic.
- Identity: `check_identity_policy` against server `[identity]` config.
  Device id is required by default; the client sends an ephemeral id marked
  confidence `low`. Keyed HMAC device attestation stays PARTIAL.
- Entitlement: free-by-default `EntitlementResult`; package grants PARTIAL.
- Deferrals: `DeferralSession` with a ban-check gate; waiting/deny paths send
  `DeferralUpdate`/`Reject`, never hang (watchdog `ALD-QUEUE-TIMEOUT`).
- Queue: real `ald-queue` join; `Queued { position }` or `Admitted`.
- Manifest: `ManifestOffer` carries name + SHA-256 + size of each discovered
  `ald_manifest.toml`. Chunk transport / CDN stays PLANNED.

## Resource execution (real, Lua slice)

`LifecycleSupervisor::transition_start` parses the manifest, creates a
sandboxed `LuaRuntime` with the `Aldivine.Events` surface, and executes every
`server_scripts` entry. Success records handle count and marks `Running`;
any failure marks `Failed`. `spawn` loads for real (`spawn_scripts_load`).
The Lua host is a declared native boundary: `ald-server` enables it only via
`--features lua` (default closure stays C-free, `audit-native` green);
without the feature, start fails honestly instead of pretending.

## Aegis API (real, local)

`ald-server` serves Axum on `127.0.0.1:40120` (non-loopback binds refused):

- `GET /health`, `GET /status`, `GET /resources`
- `POST /resources/:name/:action` (start/stop/restart) drives the real
  lifecycle channel.

React UI, first-run wizard, and auth stay PLANNED.

## Honest blockers

- GTA present on this box: Epic install detected earlier at
  `C:\Program Files\Epic Games\GTAV`. Game Bridge / Native Bridge / NUI /
  F8 / asset runtime stay BLOCKED_EXTERNAL until exercised against the game.
- Asset formats: recognition PASS (synthetic); content validation, mount
  into the game, and rendering BLOCKED_EXTERNAL.
- DB: capability/conformance suites green; live Postgres/MySQL/MariaDB
  conformance BLOCKED_EXTERNAL (no DB server here).
- Node/CLR: resolution/authorization halves only; execution BLOCKED_EXTERNAL.
- Voice audio transport: BLOCKED_EXTERNAL.
