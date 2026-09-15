# Security Model

Aldivine assumes users legitimately own GTA V. It never implements piracy, DRM/credential
theft, or anti-cheat bypass.

## Trust Boundaries

1. **Client is untrusted.** All gameplay data is validated server-authoritatively.
2. **Resources are partially trusted.** Each declares capabilities; the runtime denies
   undeclared access (filesystem, network, db, player, entity, identity reads).
3. **Admins are privileged.** Aegis RBAC scopes every action; all privileged actions audited.

## AstraNet Firewall

- packet validation before gameplay code;
- replay protection (sequence + session);
- authentication + session tokens;
- event ACL per resource capability;
- anti-spam / anti-flood (global + per-event rate limits);
- structured security logs (no secrets).

## Identity & Privacy

- AldivinePlayerId (ULID) is primary key; external IDs are linked, never primary.
- Device ID: keyed HMAC over normalized local signals. Raw serials stay local, never
  transmitted, never shown to Lua/JS or admins. Aegis sees only device id/version/confidence.
- IP: authoritative server-observed value; client-reported IP ignored. Proxies trusted only
  from configured `trusted_proxies` CIDRs.
- Entitlement: never reports VERIFIED without legitimate validation; otherwise UNKNOWN.

## Secrets

- No secrets committed. `.env.example` documents required vars; `.env` gitignored.
- Redact: access_token, refresh_token, authorization header, session cookie, password,
  db password. Log redaction tested automatically.

## Threat Model

See docs/security/THREAT_MODEL.md.
