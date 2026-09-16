# Privacy

Aldivine collects the minimum identity signals required to operate a fair multiplayer
environment for legitimate GTA V owners.

## Data Used

- **Rockstar identity**: only when legitimately resolvable; status + verified flag.
- **Steam identity**: SteamID64 (canonical) + compatibility `steam:hex`. Never trusted from
  client; validated via session where possible.
- **Epic identity**: only when legitimately available.
- **Device ID**: privacy-preserving keyed fingerprint (`device:v1:<hmac>`). Raw motherboard /
  disk / CPU / TPM / MAC serials are NEVER collected, transmitted, or exposed.
- **Connection IP**: server-observed remote address; used for rate limiting / ban signal
  (weak signal only).

## What We Do NOT Do

- Never steal passwords, cookies, launcher tokens, or browser sessions.
- Never expose raw hardware identifiers to resources or administrators.
- Never treat same-IP as same-user (NAT/CGNAT/VPN/household).

## Retention

- Identity links, connection history, sessions, and bans retained per server policy.
- Aegis shows administrators only: Aldivine ID, provider statuses, SteamID64, Steam Hex,
  Device ID + confidence, first/last seen, connection IP, session history.
- Account deletion / anonymization strategy: provider identifiers dissociated on request
  where technically feasible (documented in server policy).

## Administrator Permissions

Aegis RBAC gates `players.identity.read`, `players.identity.device.read`,
`players.identity.network.read`. Raw serials are not present in any admin view.

## Telemetry Consent & Data Minimization

1. **Default-Deny Policy**: Telemetry collection defaults to `Unset` / blocked until the player explicitly opts in.
2. **Granular Categories**:
   - `PerformanceMetrics`: Local FPS, tick time, memory usage.
   - `CrashReporting`: Anonymized stack traces and crash dumps with PII redacted.
   - `UsageAnalytics`: Feature interactions and launcher launch events.
   - `NetworkDiagnostics`: Latency percentiles and packet loss rates.
3. **Immediate Opt-Out**: If consent is set to `OptedOut`, all collection pipelines discard metrics unconditionally.
4. **Localization & Accessibility**:
   - Interface localization respects user locale preferences (`en-US`, `id-ID`, etc.) without transmitting language preferences to third parties.
   - Accessibility settings (high contrast, text scale, colorblind adjustments) remain strictly client-side local settings.
