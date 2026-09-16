# Aldivine Edge Reverse Proxy & Gateway

Aldivine Edge (`crates/ald-edge`) provides pre-authentication, ingress packet inspection, and DDoS absorption.

## 1. Responsibilities

- **Pre-Authentication**: Validates connection tokens before forwarding packets to the core game server.
- **DDoS Mitigation**:
  - Per-IP connection and bandwidth rate limits
  - Synchronous SYN flood and packet fragmentation filtering
  - Dropping malformed or oversized datagrams (> 16 MiB) at the edge
- **Forwarding**:
  - Secure UDP proxying to `ald-server` with forwarded client IP encapsulation and shared HMAC authentication secrets.
