# NODE MATRIX

Exact-version certification for the Aldivine Node Compatibility Runtime.

**No row in this file is PASS yet.** A row may only be marked PASS with the
engine version, Aldivine version, the corpus used, and a date — and only when
that run actually happened. See `docs/NODE_COMPATIBILITY.md` for the
BLOCKED_EXTERNAL blocker.

## Certification rows

| Profile | Engine | Aldivine | Corpus | Result | Date |
|---------|--------|----------|--------|--------|------|
| node16 | not vendored | 0.1.0 | — | BLOCKED_EXTERNAL | — |
| node22 | not vendored | 0.1.0 | — | BLOCKED_EXTERNAL | — |

## What is verified today (engine-independent)

These are tested behaviours of `crates/ald-script-node`, each with a test in
that crate. They are compatibility *machinery*, not Node compatibility claims.

| Behaviour | Status |
|-----------|--------|
| `node_version` manifest directive → profile selection | PASS |
| Unknown node major rejected instead of defaulted | PASS |
| `engines.node` semver enforcement per profile | PASS |
| Relative/absolute specifier resolution with extension probing | PASS |
| Directory resolution via `package.json` `main`, then `index.*` | PASS |
| `node_modules` upward walk | PASS |
| Scoped package + subpath resolution | PASS |
| Unresolvable specifier is an error, never a silent no-op | PASS |
| Specifier traversal cannot escape the resource root | PASS |
| Locked versions win; unsatisfied ranges reported | PASS |
| Deterministic plan order regardless of lockfile order | PASS |
| `sha512`/`sha256` integrity parse + verify; other algorithms refused | PASS |
| Content-addressed cache path dedupes identical bytes | PASS |
| Install scripts disabled by default, opt-in only | PASS |
| Native addons blocked by default; arch-checked when permitted | PASS |
| Built-in capability table (30 entries, no duplicates) | PASS |

## Capability-gated built-ins

Reachability requires the host to grant the capability. A resolvable module is
still refused at `require()` without the grant.

| Built-in | Requirement |
|----------|-------------|
| `assert`, `buffer`, `events`, `path`, `querystring`, `string_decoder`, `url`, `util`, `punycode` | none (pure computation) |
| `crypto` | `node.crypto` |
| `fs`, `fs/promises`, `os` | `node.fs` |
| `net`, `http`, `https`, `http2`, `dns`, `tls`, `dgram` | `node.net` |
| `process` | `node.process` |
| `child_process`, `cluster`, `worker_threads`, `vm` | `node.child_process` |
| `module`, `repl`, `inspector`, `v8`, `perf_hooks`, `trace_events`, `diagnostics_channel` | Denied — never reachable |

## Profile semantics

| Property | node16 | node22 |
|----------|--------|--------|
| Presented `process.version` | 16.20.2 | 22.11.0 |
| Global `fetch` | absent | present |
| `node:` specifier prefix | not recognized | recognized |
| Execution | BLOCKED_EXTERNAL | BLOCKED_EXTERNAL |
