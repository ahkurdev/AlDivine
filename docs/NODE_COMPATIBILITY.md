# NODE COMPATIBILITY

Status: **PARTIAL** — engine-independent half IMPLEMENTED, evaluation
BLOCKED_EXTERNAL.

QuickJS is not Node.js, and this project does not pretend it is. FiveM server
JavaScript resources are written against Node semantics: `require()`,
`node_modules`, `package.json`, `setImmediate`, `setTick`, Node built-ins,
`node_version`. Aldivine's client JS runtime (`crates/ald-script-js`) provides
none of those on purpose — a legacy server script that reaches for `require`
there gets a `ReferenceError`, which is the honest failure.

`crates/ald-script-node` is the separate compatibility runtime for scripts that
genuinely need Node.

## What is implemented

| Area | Status | Detail |
|------|--------|--------|
| Compatibility profiles | IMPLEMENTED | `node16` (legacy) and `node22` (modern), selected from the manifest `node_version` directive via `select_profile`. Unknown majors are rejected, not defaulted. |
| Profile semantics | IMPLEMENTED | `has_global_fetch` and `has_node_prefix` differ per profile, so a script relying on a Node-16/Node-22 difference is judged against the right semantics. |
| `package.json` | IMPLEMENTED | `dependencies`, `devDependencies`, `optionalDependencies`, `scripts`, `engines`, `cpu`, `os`, `main`. |
| `engines.node` enforcement | IMPLEMENTED | Real semver matching against the profile's presented version (16.20.2 / 22.11.0). A package whose engine range the profile cannot satisfy is refused up front. |
| Module resolution | IMPLEMENTED | Node's algorithm: relative/absolute probing, `.js`/`.json`/`.node` extension order, directory handling (`package.json` `main` then `index.*`), `node_modules` upward walk, scoped names, subpaths. |
| Dependency resolution | IMPLEMENTED | Locked versions always win (no silent upgrade); unsatisfied ranges are reported, sorted canonically so the plan is byte-identical regardless of lockfile order. |
| Integrity | IMPLEMENTED | npm-style `sha512-`/`sha256-` base64 integrity parsing and verification. Other algorithms are refused — no invented cryptography. |
| Content-addressed cache path | IMPLEMENTED | `cache/content-v2/<algo>/<h[0..2]>/<h[2..4]>/<h[4..]>`; identical bytes dedupe by construction. |
| Install policy | IMPLEMENTED | Install scripts OFF by default; `.node` native addons refused unless explicitly permitted, and then architecture-checked against declared `cpu`. |
| Built-in capability table | IMPLEMENTED | 30 built-ins classified as `Allowed` (pure computation), `CapabilityGated` (fs/net/process/child/threads/vm), or `Denied` (module, repl, inspector, v8, perf_hooks, trace_events). |
| JavaScript evaluation | BLOCKED_EXTERNAL | No JS engine is vendored in this build. `require()` is resolved and authorized here; the module body is not executed by this crate. |

## BLOCKED_EXTERNAL — exact blocker

Executing a resolved module requires a JavaScript engine with Node-compatible
host objects (`process`, `Buffer`, `module.exports` semantics, the event loop,
`setImmediate` ordering, `node:` built-in shims) and, for compatibility claims,
the ability to run the same script under both Node 16 and Node 22 semantics.

Blockers, precisely:

1. **No engine in the build.** The workspace vendors `rquickjs` 0.13 (QuickJS,
   bindgen). QuickJS is not a Node-compatible host and this project will not
   label it one. A separate engine integration (V8 or the genuine Node binary
   as a sidecar) is what the evaluation half requires.
2. **No Node binary is distributed.** Shipping Node inside Aldivine means
   owning its security updates and license obligations; that is a product
   decision, not an engineering one, and it is not taken here.
3. **Version-semantics parity is unverifiable without both engines.** Claiming
   "Node 16 PASS / Node 22 PASS" requires actually running the corpus on both.
   Neither engine is present, so no such claim is made.

Per the master spec, the blocked part is: isolated (this crate has no engine
dependency), abstracted (resolution/authorization behind `NodeFileSource` and
`InstallPolicy`), test-doubled (in-memory `MemTree` used by every resolution
test), and documented here — then independent work continued.

## Consequence for resources

A resource whose manifest declares `node_version` and whose scripts `require()`
things is **NOT_SUPPORTED** for execution today. It is not silently degraded
and it is not run under QuickJS-with-lies. It fails with `NodeError` /
`ReferenceError` and the reason is shown in Aegis and the F8 console.

## Capability model

Scripts never receive ambient authority. A resolved `require('fs')` is only
satisfiable when the host has granted `node.fs` to that resource; without the
grant the `require` call is refused even though the module is resolvable.
Gating is by capability string (`node.fs`, `node.net`, `node.process`,
`node.child_process`, `node.crypto`) so a resource's manifest declares what it
needs and nothing more.

`Denied` built-ins are never reachable from a resource regardless of grants:
`module`, `repl`, `inspector`, `v8`, `perf_hooks`, `trace_events`,
`diagnostics_channel`. These expose either the host's internals or an
unbounded introspection surface, and no resource has a legitimate use for them
inside a game server.

## Test evidence

`cargo test -p ald-script-node` — 33 tests. The set that matters:

- `integrity_verifies_real_digests` — sha512/sha256 of `b"hello"` against
  independently known base64 digests, plus a one-byte tamper rejection.
- `base64_decoder_matches_known_encoding` — decoder cross-checked against the
  same reference encodings and an unpadded input.
- `bare_walks_up_to_parent_node_modules` — the upward walk actually walks.
- `resolves_scoped_package_and_subpath` — `@scope/pkg` and `@scope/pkg/lib/deep`.
- `traversal_in_specifier_cannot_escape_the_resource_root` — `../../etc/passwd`
  and a deeper relative escape both fail to resolve.
- `install_scripts_are_disabled_by_default` — a `postinstall` pulling a remote
  script is refused, and refused only because the policy says so (opt-in passes).
- `native_addon_arch_is_checked_when_permitted` — an `arm64`-declared addon is
  refused on an `x86_64` host even with addons permitted.
- `plan_is_deterministic_regardless_of_lock_order` — same plan from shuffled locks.
- `engines_are_enforced_against_profile` — `>=20` passes on Node22, fails on Node16.

## Upgrade path

To move evaluation out of BLOCKED_EXTERNAL:

1. Vendor a real JS engine (V8 via `rusty_v8`, or the Node binary as a
   supervised sidecar with its own process boundary — the sidecar is the safer
   default since a resource crash must not take the server down).
2. Implement the Node host objects this crate's capability table already names,
   enforcing the table at `require()` time.
3. Run one authored corpus under both profiles and record the results in
   `docs/compatibility/NODE_MATRIX.md` as exact-version certification.
4. Only then may a "Node 16 PASS / Node 22 PASS" row appear anywhere.
