# Aegis Server Management & Control Plane

Aegis is the official operations platform, administration interface, and process manager for Aldivine servers.

## 1. Architecture

- **Aegis Node Agent (`agent/aegis-node`)**: Background process supervisor isolating process management from browsers.
- **Aegis Backend (`aegis/backend`)**: Rust + Axum API providing RBAC, metrics, audit logs, and instance controls.
- **Aegis Web (`apps/aegis-web`)**: React + TypeScript administration interface styled in Sky Blue.

## 2. Core Capabilities

- First-run wizard and server provisioning
- Live resource manager (start, stop, restart, hot reload, quarantine)
- Live server console with remote command channel
- Performance profiler (P50/P95 tick latencies, CPU, RAM)
- Database capability monitor and schema migration manager
- Asset streaming and cache inspection
- Safe mode crash loop recovery dashboard
