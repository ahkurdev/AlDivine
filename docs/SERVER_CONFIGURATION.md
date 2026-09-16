# Aldivine Server Configuration (`server.toml`)

`server.toml` is the native, structured configuration format for `ald-server`.

## 1. Schema Overview

```toml
[server]
name = "Aldivine Community Server"
max_players = 64
description = "Production roleplay server"
game_build = 3095

[network]
bind_udp = "0.0.0.0:30120"
bind_tcp = "0.0.0.0:30120"
protocol_version = 1

[security]
max_connections_per_ip = 8
enable_firewall = true
global_event_rate_limit = 1000

[identity]
require_device_id = true
require_aldivine_account = false

[runtime]
worker_threads = 4
max_resources = 256
```

## 2. Validation

Validate syntax and semantics via CLI:
```bash
ald config validate server.toml
```
Enforces type constraints, IP formatting, thread bounds, and port allocations.
