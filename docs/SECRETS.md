# Aldivine Secrets Abstraction Specification

Project Aldivine enforces strict secret separation across configuration, runtime state, and logs.

## 1. Secret Sources

Secrets are defined in `server.cfg` or `server.toml` using references rather than raw plaintext values:
- `env:VARIABLE_NAME`: Read from host environment variable
- `vault:KEY`: Read from encrypted local vault or external HashiCorp Vault
- `kms:KEY_ID`: Cloud KMS secret envelope

## 2. Redaction & Isolation Guarantees

1. **Log Redaction**: `crates/ald-security` automatically redacts database passwords, RCON passwords, license keys, and bearer tokens.
2. **Memory Protection**: Secrets are never replicated over AstraNet, never exposed in client state bags, and never returned in public server query endpoints (`/info.json`, `/players.json`).
3. **Diagnostics**: `ald report create` scrubs all credentials, replacing them with `[REDACTED]`.
