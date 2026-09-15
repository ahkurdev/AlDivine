# Threat Model

Scope: Aldivine multiplayer platform (launcher, client, server runtime, networking,
resources, scripts, identity, admin).

## Assets

- Player identity & entitlement state
- Server-authoritative game state
- Administrative control plane (Aegis)
- Resource code & configuration

## Adversaries / Threats

| Threat | Vector | Mitigation |
|--------|--------|------------|
| Malicious client | Forged gameplay packets | Server-authoritative validation; reject malformed pre-gameplay |
| Packet spoofing / replay | Captured packets | Session tokens, sequence + timestamp, replay window |
| Network flooding | SYN/event floods | Per-IP + per-event rate limits; MTU aware; astranet firewall |
| Event flooding | Resource sends millions of events | Per-resource budget + runaway detector |
| Malicious resource | Escapes sandbox, reads secrets | Capability enforcement; unknown capability denied |
| Resource tampering | Modified asset | Signed manifests; checksum verification (NovaGate) |
| Admin compromise | Stolen Aegis creds | TOTP 2FA, session invalidation, brute-force protection, audit |
| Credential theft | Phish / token leak | Never log secrets; redaction tested; secure cookies; CSRF |
| SQL injection | Untrusted query params | Parameterized queries; repository layer; no string concat SQL |
| XSS | Resource UI | Sandboxed UI; validate every Runtime↔UI message; CSP |
| CSRF | Admin actions | Anti-CSRF tokens where applicable |
| Directory traversal | Path in resource read | Capability filesystem.read scoped to resource dir |
| Command injection | Unsafe shell | No shell execution in resources; deny capability by default |
| DoS | Resource runaway / alloc bomb | Budgets, bounded queues, slow-handler detection |
| Session hijacking | Stolen session cookie | Secure, HttpOnly, short-lived; rotate on privilege change |
| Identity spoofing | Fake Steam/RS id | Validate via platform/session APIs; never trust client-sent id |

## Assumptions / Limits

- Legitimate GTA V ownership assumed; no DRM bypass implemented.
- Some entitlement verification requires platform APIs unavailable locally → ENTITLEMENT_UNKNOWN.
- Raw hardware serials out of scope for collection (privacy).
