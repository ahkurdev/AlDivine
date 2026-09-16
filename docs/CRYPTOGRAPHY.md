# Aldivine Cryptography & Key Management Specification

Project Aldivine strictly enforces standard, vetted cryptographic primitives. Custom or proprietary cryptography is prohibited.

## 1. Cryptographic Primitives

| Purpose | Algorithm / Standard | Implementation / Library |
|---|---|---|
| **Package & Release Signatures** | Ed25519 (RFC 8032) | `ed25519-dalek` v2 |
| **Content Addressing & Checksums**| SHA-256 (FIPS 180-4) | `sha2` crate |
| **Transport Layer Security** | TLS 1.3 / QUIC | `rustls` (where applicable) |
| **Identity Token Keyed Hashes** | HMAC-SHA256 | `hmac` / `sha2` |
| **Password Hashing** | Argon2id | `argon2` crate |
| **Asset Cache Integrity** | SHA-256 | `sha2` crate |

## 2. Policy: No Custom Cryptography

- Never invent or roll proprietary ciphers, hash functions, or key exchange protocols.
- Use mature, audited Rust crates (`sha2`, `ed25519-dalek`, `argon2`).
- Security takes precedence over language purity: vetted standard implementations are mandatory.

## 3. Key Hierarchy & Protection

- **Root Release Key**: Offline Ed25519 private key signing official client & server release manifests.
- **Publisher Keys**: Ed25519 keypairs signing `.alpkg` package bundles.
- **Session Tokens**: Cryptographically random 256-bit entropy ephemeral session keys.
- **Revocation**: Key revocation lists maintained by `ReputationAuthority` in `ald-package-scanner`.
