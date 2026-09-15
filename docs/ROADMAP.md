# ROADMAP

Dependency-ordered phase plan. Each phase compiles, tests, and documents before the next begins.

| # | Phase | Status |
|---|-------|--------|
| 0 | Architecture documentation | IMPLEMENTED |
| 1 | Rust workspace + foundational crates | PARTIAL (9 crates written; MSVC toolchain install pending) |
| 2 | Configuration & error architecture | IMPLEMENTED (ald-config, ald-core) |
| 3 | Aldivine Protocol | IMPLEMENTED (ald-protocol: header, channels, flags, event codec) |
| 4 | AstraNet transport | PARTIAL (framing done; transport sockets pending) |
| 5 | Identity abstraction | PARTIAL (ald-identity, ald-device-identity) |
| 6 | Resource system | IMPLEMENTED (manifest, lifecycle, capabilities, dep resolution) |
| 7 | Event & RPC systems | IMPLEMENTED (ald-events, ald-rpc) |
| 8 | Aldivine Scheduler | PARTIAL (policy/budget/detector; async executor pending) |
| 9 | Lua runtime | PLANNED |
| 10 | JavaScript runtime | PLANNED |
| 11 | Server runtime | PLANNED |
| 12 | Entity architecture | PLANNED |
| 13 | State replication | PLANNED |
| 14 | Astryn client foundations | PLANNED |
| 15 | Aldivine SDK | PLANNED |
| 16 | Aldivine Framework | PLANNED |
| 17 | Database & persistence | PLANNED |
| 18 | Inventory | PLANNED |
| 19 | Jobs / Orgs / Economy | PLANNED |
| 20 | Compatibility abstraction | PLANNED |
| 21 | ESX adapter | PLANNED |
| 22 | QBCore adapter | PLANNED |
| 23 | Qbox adapter | PLANNED |
| 24 | Migration tooling | PLANNED |
| 25 | NovaGate launcher | PLANNED |
| 26 | Aegis backend | PLANNED |
| 27 | Aegis frontend | PLANNED |
| 28 | Profiling | PLANNED |
| 29 | Security hardening | PLANNED |
| 30 | Load testing | PLANNED |
| 31 | Documentation | PLANNED |
| 32 | Packaging & releases | PLANNED |
| 33 | End-to-end validation | PLANNED |

Next action: finish compiling foundational crates once the MSVC build tools finish installing.
