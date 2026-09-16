# Astryn Developer Console (F8) Specification

The Astryn Developer Console provides runtime diagnostics, script debugging, network tracing, and memory profiling within the native GTA V multiplayer client.

## 1. Key Binding

- **Default Key**: `F8` (rebindable in user preferences).
- Reserved and protected by `ald-input`: ordinary resources cannot capture or override `F8` unless explicitly granted developer bypass.

## 2. Console Tabs

1. **Console**: Script logs, stdout/stderr, command input with autocomplete and command history.
2. **Resources**: Active resource package list, lifecycle state, memory footprint, restart triggers.
3. **Errors**: Unhandled exceptions, stack traces with sourcemap resolution.
4. **Network**: AstraNet channel throughput, RTT, packet loss, bandwidth graphs.
5. **Downloads**: Streaming engine download lanes, chunk reassembly, active CDN mirrors.
6. **Assets**: Asset format mount states, memory residency status, cache hit rates.
7. **Performance**: Frame time graph, client tick duration, memory allocations.
8. **Entities**: Local and networked ECS entities, network IDs, ownership assignments.
9. **Natives**: Native invocation logs, call counts, return values.
10. **State**: Live state bag inspector (GlobalState, LocalPlayer, Entity states).
11. **NUI / DUI**: Active offscreen browser instances, texture handles, message queues.
12. **Developer Tools**: Telemetry monitors, physics wireframes, hitboxes.

## 3. Security Boundary

The developer console is client-local and does NOT grant administrative or server RCON privileges by default.
