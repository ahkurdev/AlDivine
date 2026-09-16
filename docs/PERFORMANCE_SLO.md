# Aldivine Performance Service Level Objectives (SLO)

This document establishes measurable performance targets and benchmarking methodologies for Project Aldivine. Claims of "fast" or "zero overhead" are strictly prohibited without verified benchmark data.

## 1. Client Runtime (Astryn Client)

| Metric | Target (SLO) | Measurement Method | Failure Boundary |
|---|---|---|---|
| **Astryn Idle Overhead** | < 1.5% CPU, < 45 MB RAM | Process baseline during in-world idle | > 3.0% CPU / > 80 MB |
| **F8 Console Open Latency** | < 16 ms (sub-1-frame at 60 FPS) | Keypress to UI viewport render | > 33 ms |
| **Streaming Frame-Time Impact** | < 2.0 ms added frame time | Delta frame time during active asset mount | > 5.0 ms (causes stutter) |
| **Mount Commit Duration** | < 8 ms per game-thread batch | Asset registration commit time | > 16 ms |
| **Download Stall Recovery** | < 3.0 s to resume or switch | Download watchdog virtual tick poll | > 5.0 s |

## 2. Dedicated Server Runtime (Aldivine Server)

| Metric | Target (SLO) | Measurement Method | Failure Boundary |
|---|---|---|---|
| **Server Tick P50** | < 2.5 ms | Server main loop tick duration | > 5.0 ms |
| **Server Tick P95** | < 6.0 ms | Under 64 active players load | > 10.0 ms |
| **Server Tick P99** | < 12.0 ms | Peak load with physics and script events | > 20.0 ms |
| **Reliable Event Latency** | < 15 ms local dispatch | Ring buffer queue through scheduler | > 50 ms |
| **AstraNet Queue Depth** | < 256 pending packets/channel | Channel backpressure monitor | > 1024 packets |
| **Resource Startup Time** | < 250 ms for canonical pack | Measured across all 7 base resources | > 1000 ms |

## 3. Storage, Database & Control Plane

| Metric | Target (SLO) | Measurement Method | Failure Boundary |
|---|---|---|---|
| **Database Query P95** | < 10.0 ms | Relational query latency (PG/MySQL/MariaDB) | > 25.0 ms |
| **Aegis API Interaction Latency**| < 50 ms | Node agent REST/WebSocket response time | > 150 ms |
| **Server Memory Overhead** | < 120 MB baseline | `ald-server` with 7 base resources active | > 250 MB |

## 4. Benchmark Execution

Automated benchmarks run via:
```bash
cargo bench --workspace
```
Continuous calibration is verified against these bounds before any release candidate is approved.
