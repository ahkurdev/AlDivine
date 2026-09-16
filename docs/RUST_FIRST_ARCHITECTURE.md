# Rust-First Architecture

Aldivine core systems prefer pure Rust implementations whenever doing so
does not materially reduce correctness, compatibility, security,
performance, maintainability, or platform integration quality.

## Rust-first core (non-exhaustive)

AstraNet protocol logic, scheduler, event bus, RPC, resource manager,
server.cfg parser, NormalizedServerConfig, VFS, permissions/RBAC, state
system, state replication, ECS, entity ownership, interest management,
world sync helpers, asset dependency checks, streaming scheduler, chunk
planning, resume logic, download watchdog, content-addressed cache, cache
repair, hashing orchestration, telemetry, tracing, configuration, package
management, queue/deferral policy, persistence journal, snapshot framing.

## Isolated native boundaries

Documented in `docs/NATIVE_DEPENDENCY_POLICY.md` and enforced by
`ald dependencies audit-native`. The compat runtimes (JS engine, Lua,
native extension loading, and in future CEF, CLR, Node) live in their own
crates and load only when selected. The native path ships none of them —
verified, not asserted.

## Principle (not a purity contest)

Security maturity outranks language purity: a vetted provider using a
carefully isolated native library beats an immature pure-Rust option.
"Zero C dependencies" is never a release claim; the audit result is.
