# DEVLOG

## 2026-09-15 — Phase 0 + Phases 1-8 foundational crates

- Created Cargo workspace with 9 crates: ald-core, ald-config, ald-protocol,
  ald-device-identity, ald-identity, ald-events, ald-rpc, ald-resource, ald-scheduler.
- Each crate ships unit tests (parse/roundtrip/validation/leak-detection/runaway).
- Implemented: config validation, protocol header+channels+flags, event codec,
  keyed device fingerprint, identity aggregation + ban-confidence, namespaced event bus
  with protected namespaces, RPC registry with timeouts, resource manifest/lifecycle/
  capabilities/dep-resolution, scheduler workload classes + per-resource budgets +
  runaway detector.
- Docs written: ARCHITECTURE, ROADMAP, IMPLEMENTATION_STATUS, PROTOCOL, SECURITY_MODEL,
  PRIVACY, DEVLOG, security/THREAT_MODEL.
- Build blocker discovered: MSVC `link.exe` absent on host; `/usr/bin/link` shadows it.
  Interim `.cargo/config.toml` routes MSVC target through bundled `rust-lld`. This alone
  still needs MSVC import libs (kernel32.lib etc.), so installing Visual Studio 2022
  Build Tools + Windows 11 SDK via winget. First `cargo test` runs after install completes.
- Next: compile + run all crate tests, then proceed to Phase 4 transport (ald-network),
  Phase 9/10 script runtimes, Phase 11 server runtime.

## 2026-09-15 — Master spec alignment + PHASE 5 + PHASE 6

- Adopted the ULTIMATE MASTER SPECIFICATION as single source of truth; ROADMAP
  rewritten to the 74-phase model with honest per-phase statuses.
- Baseline verified before new work: full workspace `cargo build` green and
  336 tests / 0 failures across 68 test binaries.
- Fixed a real environment bug: the session PATH leaked an incompatible
  MSVC layout (Program Files/ x86 absent, cl.exe exit 0xc0000017). Builds now
  run through `.env-build.sh`, which sources the installed
  14.44.35207 toolchain + Windows 11 SDK 10.0.22621.0 explicitly.
- PHASE 5 (time sync / negotiation / query): ald-timesync (NTP-style offset +
  RTT + jitter, clamped interpolation delay, server-authoritative clocks,
  client time-claim bounds), ald-negotiation (server-authoritative feature
  negotiation with typed outcomes and structured deprecation UX:
  ClientTooOld / ServerTooOld / Incompatible carrying UpgradeAction),
  ald-query (ALDQ probe protocol, bounded JSON with progressive shedding,
  unlisted servers answer counts only, anti-amplification rate limiter,
  legacy /info.json /players.json /dynamic.json from one ServerStatus).
- PHASE 6 (Aldivine Edge): pre-auth admission with per-IP and origin
  connection caps, banned IP/CIDR, query budget isolated from gameplay,
  origin shielding (fail_closed + shield_allow_networks), and HMAC-SHA256
  authenticated real-IP forwarding with freshness window + constant-time
  compare. SHA-256/HMAC are stdlib-only and verified against FIPS 180-2 and
  RFC 4231 TC2 vectors rather than asserted by inspection.
- 49 new tests (10 timesync, 11 negotiation, 13 query, 15 edge). All green.
- Next: PHASE 8 ald-vfs (@resource/path, sandbox writes, symlink/junction
  and reparse-point hardening, case-insensitivity, Unicode normalization,
  reserved Windows device names).

## PHASE 8 — Aldivine VFS (ald-vfs)

- Implemented resource-scoped virtual filesystem: mount table, @resource/path
  resolution, explicit one-way share() for cross-resource reads, sandboxed writes.
- Hardening, all backed by tests: lexical traversal bound (net depth < 0 rejected),
  Windows reserved device names (COM1-9 / LPT1-9 / con / prn / aux / nul),
  alternate data streams, NUL bytes, RTLO override and combining diacritics.
- Reads and writes re-canonicalize after following filesystem links, so a
  symlink or junction swapped in after validation still cannot escape.
- Writes go to a temp sibling then rename only after final containment check.
- 16 tests green, including a 4600-case adversarial path sweep that asserts
  every generated path either errors or resolves inside the sandbox root.
- Bugs found by the tests and fixed: ..  accepted as a resource name; COM10+
  falsely treated as a reserved device name (Windows reserves COM1-9 only);
  depth check ran on raw backslash separators before normalization;
  verify_within rejected the legit Windows absolute-prefix of a canonical root.

## PHASE 9 — fxmanifest compatibility parser (ald-fxmanifest)

- Purpose-built Lua table-literal lexer + parser for fxmanifest.lua / __resource.lua.
  Deliberately NOT a Lua interpreter: it understands exactly the manifest subset
  (string/number/boolean scalars, tables, nested tables, long strings, long
  comments, escapes) and rejects bare identifiers, so a manifest containing real
  Lua code is a syntax error rather than silently mis-parsed.
- NormalizedManifest is the single shape the runtime consumes; ald_manifest.toml
  parses into the same struct.
- Every directive is classified SUPPORTED / TRANSLATED / IGNORED_WITH_WARNING /
  UNSUPPORTED and recorded with line number in source order. Unknown metadata is
  preserved verbatim — nothing is silently dropped, per the master spec.
- convar_category parses into structured Aegis form metadata (convar, name, help,
  default, min, max, type) to drive automatic resource configuration UI.
- 20 tests green; full workspace regression 511 tests, 0 failed.

## PHASE 10 — CfxLua compatibility frontend (ald-cfxlua-compat)

- Vector value types (vector2/3/4, quat/quaternion) on mlua Lua 5.4: field
  semantics, arithmetic metatables (__add/__sub/__mul both vec*vec and
  vec*scalar), __tostring, __eq.
- joaat one-at-a-time hashing as a joaat() global and as a backtick source
  preprocessor so legacy sources load unmodified.
- json global with Cfx array-vs-object table detection; msgpack global with a
  self-contained ext-free msgpack codec (golden wire bytes, truncation
  rejected).
- promise global + Citizen.Await (synchronous resolve, reject surfaces as a
  Lua error).
- Reference-value correction caught during testing: an earlier test asserted
  joaat("adder") == 0x2b41d343, which was an invented value. The true Jenkins
  one-at-a-time result is 0xb779a091, verified independently. Test fixed, not
  the implementation.
- 20/20 tests green; workspace regression 531 tests / 0 failed.

## PHASE 11 — Client JavaScript runtime (ald-script-js)

- Citizen-compatible client surface on QuickJS via rquickjs 0.13: timers,
  Citizen namespace, on/onNet/emit, TriggerClientEvent/TriggerServerEvent
  onto a host-drained wire queue, console routing, GetCurrentResourceName.
- Virtual-clock timer pump (tick_timers advances a saturating u64 clock;
  one-shots fire once, intervals reschedule at clock+period).
- Root causes fixed this phase:
  1. rquickjs 0.13 IntoJsFunc takes positional args only; single-tuple
     closures match no impl. All callbacks converted.
  2. Object::get turbofish order is get::<key, value>; Array/Object keys
     need IntoAtom (u32, not usize); Value is a struct, no enum variants.
  3. Ctx is invariant over its lifetime: a callback parameter cannot be
     stored into objects tied to a captured outer Ctx (E0521). Callbacks now
     kept in Rust-side registries as GC-rooted Persistent handles, taken as
     Persistent<Function<'static>> params via its FromJs impl.
  4. Ctx::clone runs JS_DupContext: capturing a Ctx in a JS-bound closure
     holds a context ref that deadlocks teardown and trips the debug
     GC-empty assert (list_empty(&rt->gc_obj_list)). Calling context is now
     a per-call Ctx parameter (FromParam, consumes no JS args); closures
     capture only plain Rust state.
  5. Explicit Drop clears rooted registries while the heap is live; field
     order is state/ctx/runtime so roots and heap die before the runtime.
- require/process/Buffer deliberately absent (ReferenceError on touch).
- 16/16 tests green; no teardown abort.

## 2026-09-16 — PHASE 12: ald-script-node (Node 16/22 compatibility runtime, engine-independent half)

Scope decision (honesty first): no Node.js engine exists in this offline
environment, so the crate implements ONLY the engine-independent half and
explicitly does not claim `require`/`process`/`Buffer` evaluation.

Implemented:
- Node compatibility profiles: node16 (legacy) / node22 (modern), selected
  from the normalized manifest `node_version` directive.
- `engines.node` semver enforcement against the selected profile.
- package.json parsing (name, version, main, dependencies, scripts, engines).
- Node module resolution mirroring Node's algorithm shape: relative/absolute,
  directory index, package.json `main`, node_modules walk, extension probing.
- Native addon detection (`.node` reached through `main`) flagged for policy.
- Lockfile / integrity (sha512 base64 via sha2+base64), deterministic resolve.
- Install-script policy gate: DISABLED BY DEFAULT, explicit permission required.
- Virtual-clock timers (setTimeout/setInterval/setImmediate/setTick/clearTick)
  and the citizen event surface (on/onNet/emit/emitNet/addRawEventListener).

Tests: 33/33 green. Workspace regression: 577 passed / 0 failed / 84 binaries.

Docs:
- docs/NODE_COMPATIBILITY.md — Status PARTIAL; evaluation BLOCKED_EXTERNAL.
- docs/compatibility/NODE_MATRIX.md — exact-version certification matrix;
  no row is PASS yet (requires engine version + Aldivine version + evidence).

## 2026-09-16 — PHASE 13: ald-script-dotnet (CfxCLR / .NET load-and-authorize half)

Scope decision (honesty first): no CLR is embedded in this build
(no Mono, CoreCLR, or .NET Framework host; `dotnet` absent offline), so the
crate implements ONLY the engine-independent half and explicitly does not
claim CIL execution, Tick dispatch, or event binding.

Implemented:
- Dependency-free ECMA-335 metadata reader: PE → CLI header → metadata
  streams → tables, recovering runtime version, CLI flags, module name,
  MVID, assembly identity, AssemblyRefs, TypeDefs with base types,
  P/Invoke imports, custom-attribute type names. Verified against genuine
  on-disk framework assemblies (`mscorlib.dll`, `System.Core.dll`), never
  synthetic bytes pretending to be ones.
- BCL target inference (System.Private.CoreLib/System.Runtime → CoreClr,
  mscorlib → NetFramework, Mono markers → Mono) with host-flavor refusal
  instead of pretend-load.
- CLR host profiles `Mono` (FiveM-era) / `CoreClr` (new deployments),
  selected from normalized manifest `clr_disable_task_scheduler`.
- Transitive dependency resolution: name match, downgrade refused, upgrade
  recorded (never hidden), explicit public-key-token trust list, topological
  load plan, cycle reported with members.
- Ordered resource authorization: BCL mismatch → CitizenFX.Core reference →
  BaseScript subclass → signature policy → native-call policy.
- Virtual-clock `TickPump` (30 ms boundaries) pinning tick ordering semantics
  so future engine integration is wiring, not design.

Root causes fixed this phase:
1. Shared test fixture built only the tail gates' needs (no BCL reference),
   so target inference returned Unknown and every test failed at the BCL
   gate. Fixture now carries the BCL reference the first gate requires.
2. Clippy derivable-impl + single-pattern-match lints in new code fixed by
   hand (no `cargo clippy --fix`, which would strip cfg(test) imports).

Tests: 20/20 green. Workspace regression: 597 passed / 0 failed / 86 binaries.

Docs:
- docs/DOTNET_COMPATIBILITY.md — Status PARTIAL; CIL execution BLOCKED_EXTERNAL.
- docs/compatibility/DOTNET_MATRIX.md — exact-version certification matrix;
  no row is PASS yet (requires CLR version + Aldivine version + evidence).

## 2026-09-16 — PHASE 14: ald-native-extension (Native Extension ABI)

Scope: ordinary resources may not load arbitrary native binaries by
default. The crate implements authorize-then-load: manifest shape, ABI
negotiation, hash-bound Ed25519 trust, capability policy — then and only
then dlopen + `ald_extension_init` call. Loaded code runs in-process;
the gate keeps untrusted bytes out, it does not sandbox trusted code.
That boundary is documented in the module docs, not pretended.

Implemented:
- `extension.toml` manifest (id, semver version, abi_version,
  publisher_id, bare-filename library, optional requires_runtime range,
  capabilities, Ed25519 signature block).
- `HOST_ABI = 1` negotiation: mismatch is hard refusal, never downgrade.
- Hash binding: sha256 of exact library bytes is part of the signed
  canonical bytes (sorted capabilities, NUL-separated fields); swapped
  binary after authorization fails re-check.
- Trust: default-deny policy (unsigned refused, no keys trusted, no caps
  granted); unknown capability tokens fail closed; trust/policy/integrity
  failures classify as quarantine, missing files and init failures as
  operational.
- Real loading via `libloading` against a compiled cdylib fixture
  (`ald-native-extension-testext`) exporting `ald_extension_init`:
  init 0 on matching ABI, code 7 on foreign ABI (asserted), missing
  symbol probed in both directions.

Root causes fixed this phase:
1. libloading 0.8 `Library::new` is now `unsafe` — wrapped in an
   explicit unsafe block with confinement justification.
2. Struct-variant `Unsigned` mis-matched as tuple variant in
   `quarantines()`; `LoadedExtension` (holds a `Library`) needed a
   manual `Debug` impl for `unwrap_err`; test-pattern partial move of
   `capability` fixed with `ref` binding.
3. Clippy `manual_is_multiple_of` in hex decoder fixed by hand.

Tests: 19/19 green. Workspace regression: 616 passed / 0 failed.
Clippy clean (0 warnings on the new crates), `cargo fmt --check` clean.

## 2026-09-16 — PHASE 15 verify: server/ald-server gap close

The runtime skeleton already existed (startup, listener, supervisor,
console, shutdown; 6 tests). Verification against the spec found two
gaps where API pretended more than it did:

1. `ServerState::console_tx()` returned a sender whose receiver was
   dropped at construction (`_console_rx`) — the Aegis remote-command
   path went nowhere. Now the receiver is held in state, taken exactly
   once by the console task, and served by a companion task alongside
   stdin; the server terminates it on shutdown.
2. `ConsoleCommand::Stop` only logged. Now it calls
   `ServerState::request_shutdown()`, which signals the shared watch
   channel every loop already watches.

`ServerState::new` now takes the shutdown sender (deriving its own
receiver via `subscribe()`), so trigger and flag can never diverge.

Tests added (3): Stop sets the shutdown flag; remote channel serves
commands, refuses a second take, and exits on shutdown; remote Stop
shuts down and ends the server task. 9/9 green, clippy clean for the
crate (remaining warnings are pre-existing in ald-protocol/ald-network),
fmt clean. Workspace regression: 619 passed / 0 failed.

Known remaining limitation (documented, not fixed here): resource
`start` marks Running without executing scripts — script-host wiring
belongs to the runtime-integration work, not this skeleton.

## 2026-09-16 — PHASE 16: ald-queue + ald-deferrals (admission logic)

No queue/deferral code existed. Created two dependency-free logic crates
(no I/O, no clock, no network) so the admission policy is testable in
isolation before any handshake consumes it.

ald-queue (12 tests): `max_players` playing slots with a `reserved_slots`
subset only reservation holders may occupy; FIFO waitlist bounded by
`max_queue_len` (0 = direct-or-reject with `ServerFull`); leave promotes
the front-most entry the reservation rule allows (FIFO except where the
rule blocks the head — covered by test); stale entries expire by
caller-supplied virtual `now_ms` (deterministic; future-dated kept as
clock-skew grace; exact-TTL boundary kept); duplicates and empty ids
rejected, unknown leave-ids report instead of panicking.

ald-deferrals (10 tests): ordered `DeferralGate`s returning
Pass/Wait/Deny; `poll()` drives gates without re-polling passed ones;
`Wait` pauses with a bounded player-facing message (empty gets a default,
long cut on a char boundary — multi-byte tail covered); `Deny` ends with
a stable `ALD-QUEUE-*` code plus gate name for Aegis correlation; poll
watchdog denies stuck sessions with `ALD-QUEUE-TIMEOUT` (`max_polls`
clamped to >= 1 so sessions are never stillborn). Gates are sync by
design: slow checks run elsewhere and flip gate state.

Status PARTIAL (honest): policy logic done, server-handshake consumption
PLANNED — `network_loop` has no session layer yet, so wiring now would
be fake integration. 22/22 green, clippy 0 warnings, fmt clean.
Workspace regression: 641 passed / 0 failed.

## 2026-09-16 — PHASE 19: ald-game-editions + ald-game-builds

Greenfield. NovaGate detects installs but types no edition and resolves
no build numbers, so the edition/build layer is new.

ald-game-editions (4 tests): `Legacy`/`Enhanced` enum, strict parse
(only the two identifiers parse — no `Unknown` variant to mishandle
later), `EditionProfile` support/refusal with reason, undeclared edition
treated as unsupported.

ald-game-builds (13 tests): data-driven registry keyed by
(edition, number); ordered assessment (edition → known → blocked →
min_astryn); malformed `min_astryn` rejected at `add`; unparsable client
versions fail closed; `EmergencyPin` opt-in fallback that refuses by
default, requires matching edition plus a known fallback build, carries
a non-droppable warning, and never overrides known verdicts (not even
`Blocked`).

Deliberately seeded with zero real build numbers: this dev box has no
GTA V, so any "certified" list would be unverified data. Tests use
synthetic 90000+ numbers proving logic only. Certification path is
documented in docs/GTA_BUILD_SUPPORT.md (Compat Lab + evidence per
entry); required docs docs/GTA_EDITIONS.md added alongside.

One fix this phase: `GameEdition` needed `Ord` for the registry
`BTreeMap` key (compile error, first-run catch).

17/17 green, clippy 0 warnings, fmt clean. Workspace regression:
658 passed / 0 failed.

## 2026-09-16 — PHASE 26: ald-state + ald-statebag-compat

Greenfield (only a negotiation feature flag mentioned statebags).

ald-state (10 tests): typed `Value`s; per-bag seq versions; `Open` /
`OwnerOnly` policy with fail-closed no-owner; key/value breadth/depth
bounds; per-key replication flags with sorted replicated snapshots;
ordered change handlers with old/new; delete reporting; registry
lifecycle.

Self-corrections this phase (all caught before claiming):
1. Removed a `MAX_DISPATCH`/recursion-limit mechanism whose trigger was
   unreachable — handlers receive only `&ChangeEvent`, so write-back is
   impossible by construction. Documented observer semantics instead of
   shipping a cap that could never fire.
2. Fixed a wrong order-vector assertion the test run exposed ([2,3,2,3,3],
   not [2,2,3,3,3]) — expectation bug, production code was right.
3. Fixed real compile errors: or-pattern binding, double-deref filter,
   moved Rc in test.
4. Clippy lints fixed by hand (map_or → is_none_or, lazy ok_or, test
   type-complexity allow).

ald-statebag-compat (8 tests): exact scope name round-trip;
entity/player namespace isolation; 15 malformed names rejected —
including `entity:+4`, which Rust's u32 parser accepts, so signs are
rejected explicitly; compat key bound within core bound (boundary tested
both sides); strict create/write/transfer with no implicit creation.

Required doc docs/STATE_BAGS.md added. 18/18 green, clippy 0 warnings,
fmt clean. Workspace regression: 676 passed / 0 failed.

## 2026-09-16 — PHASE 27: ald-asset-registry + ald-datafiles

Recognition only, by design: no content parsing without real files to
certify against. Stream classes are Aldivine routing labels, documented
as such — not Rockstar-internal claims.

ald-asset-registry (6 tests): 11 spec-seeded extensions, recognition by
path or bare extension (case-insensitive, whitespace-trimmed),
SpecSeed→Certified upgrade with duplicate refusal, garbage registration
rejected.

ald-datafiles (6 tests): vehicle-graph seed only (the 5 filenames the
spec's asset graph states — other graphs register with their phases, not
from memory); path/case normalization; unknown-is-None; sorted
per-category listing.

Two test-exposed fixes: whitespace-padded extensions now trim in
`extension_of`; Windows drive prefixes (`C:\…`) pass the ADS colon guard
while `file:stream` and `a:b` stay rejected.

Matrix docs docs/compatibility/ASSET_FORMAT_MATRIX.md +
DATA_FILE_MATRIX.md added; every content-support cell reads PLANNED.
12/12 green, clippy 0 warnings, fmt clean. Workspace regression:
688 passed / 0 failed.

## 2026-09-16 — PHASE 25: ald-snapshot + ald-persistence

Greenfield (existing "snapshots" were network replication baselines).

ald-snapshot (9 tests): `ALDSNAP1` framing with sha256 trailer; decode
rejects wrong magic, unknown schema (both directions), truncation cut at
9 structural points, single flipped value byte (checksum), trailing
garbage, oversized entries. One expectation fix: a flip in the count
field fails at framing, not checksum — the test now pins the checksum
case to a value byte and asserts generic rejection for the header case.

ald-persistence (9 tests, all over real temp-dir files with crash
conditions simulated by truncating/flipping actual bytes): append/recover
round-trip with op application; torn tail dropped and reported; mid-file
flip fails `Corrupt(0)`; checkpoint truncates and replay continues after
it with no sequence reuse across reopen; torn checkpoint ignored with
flag; full-length bad checksum fails closed; write-path validation
without sequence consumption; fresh-dir recovery; churn compaction
verified by file size reaching 0.

Clippy: removed unused import + mut by hand. 18/18 green, clippy 0,
fmt clean. Required doc docs/PERSISTENCE.md added. Workspace regression:
706 passed / 0 failed.

## 2026-09-16 — ald-ecs: ownership transfer + dimensions

Existing store had authority labels with no rules and no isolation planes.

ownership.rs (9 tests): CAS transfer — server delegates, owners release
to server, lateral handoffs refused with `MustReleaseFirst` (migration is
release + re-acquire), CAS mismatch names the actual owner, stale ids are
`NotFound` including across slot reuse, destroy drops ownership.

dimensions.rs (6 tests): default-0 for live-unassigned, dead ids have no
plane, assignment to ghosts refused, slot reuse starts clean, `prune`
backstop with removal counts, strict same-plane visibility, live-only
membership, and the interest-set composition proven by test (same-spot
entities in different planes filter correctly).

One test bug fixed (mine): self-visibility asserted false, but same-plane
self is visible by rule — self-exclusion lives in `replication_set`.

Required doc docs/ROUTING_BUCKETS.md added. 26/26 crate green, clippy 0,
fmt clean. Workspace regression: 721 passed / 0 failed.

## 2026-09-16 — ald-population: server-side population manager

Budgets per dimension (peds + vehicles, max + density fraction,
effective cap `floor(max * density)`), deterministic by design;
unconfigured dimensions deny `NoPolicy`; tightening never culls
retroactively; releases saturate with boolean report; creation hooks
observe every decision in order.

Two test bugs fixed (mine): NaN equality in density rejection (assert by
shape), drain-count off-by-one (10 live vs cap 4 needs 7 releases to
reach 3). One clippy type-complexity fixed with a `DecisionHook` alias.

Required doc docs/POPULATION.md added. 10/10 green, clippy 0, fmt clean.

## 2026-09-16 — ald-game-events: router envelopes + decoder slots

Spec-catalog event names (unknown names fail closed everywhere:
envelope construction, subscribe, decoder registration); bounded opaque
payloads; wrap-aware per-producer dedup (RFC 1982 style, half-space jump
fails closed); ordered dispatch; unsubscribe semantics; decoder-slot
resolution exact > edition-default > unregistered with evidence refs and
duplicate refusal.

Deliberately no fake decoders: payload field decoding needs real game
bytes, so dispatch proves envelopes while typed access waits for lab
evidence. One clippy type-complexity fixed with a `Subscription` alias.

9/9 green, clippy 0, fmt clean. Workspace regression: 740 passed /
0 failed.

## 2026-09-16 — ald-lagcomp: rewind history + hit resolve

Tick-ring `HistoryBuffer` (exact/lerp-between-brackets/None — never
extrapolates; capacity prunes oldest; per-entity isolation; `forget` for
despawn cleanup); `rewind_allowed` in the tick domain (future claims fail,
budget enforced with structured errors); `hit_landed` on the horizontal
plane to agree with interest's "near". Full path test: moving target,
rewound aim lands, current-position aim misses.

Wall-clock mapping stays with ald-timesync's validated claims; damage and
verdicts stay with policy. 8/8 green, clippy 0, fmt clean. Workspace
regression: 748 passed / 0 failed.

## 2026-09-16 — ald-download: chunking, resume, watchdog, fair lanes

Transport-free control plane: `ChunkPlan` math (short tail, bad plans
rejected); `Download` per-byte coverage with idempotent overlap (only
fresh bytes advance progress), out-of-bounds and post-completion writes
rejected, `missing_ranges` resume lists, `assembled()` None until
complete; `Watchdog` virtual-tick stall reporting (boundary-tested, clock
regression safe); turn-based DRR `LaneScheduler` with exact 8/4/2/1/1
shares over 160 pops, empty-lane skip without hoarding, urgent jobs
served within a round while bulk continues.

Two real bugs caught by tests (both mine): first DRR served strict
priority (rewrote as turn-based rotation); test math assumed 20-cycles
instead of 16. One borrowck fix (copy quantum out before queue borrow).

10/10 green, clippy 0, fmt clean. Workspace regression: 758 passed /
0 failed.

## 2026-09-16 — ald-cache: content-addressed store

`objects/ab/cdef` sharded layout; stage→verify→atomic-rename commits
(idempotent on dedup hit); mismatch removes temp and errors with both
hashes; `verify_all`/`repair` with removal reports (repair deletes, never
fabricates); quota LRU evicting oldest-first with mounted protection
(protection exempts, never reserves); mtime-rebuilt recency on reopen;
stray files ignored by shape check.

Clippy fixes by hand (dead `dir` field removed, 3 unused muts, 2 clone
lints). Required doc docs/ASSET_CACHE.md added. 10/10 green, clippy 0,
fmt clean.

## 2026-09-16 — ald-mount + ald-residency: mount and ready handshake

ald-mount (8 tests): gated Queued→Registered machine over the real
format + datafile registries — unknown formats refused at queue,
hash/dependency failures retryable with reasons, illegal jumps rejected,
terminal states stick, mount point recorded.

ald-residency (8 tests, scripted bridge with call counts): virtual-tick
Requested→Waiting→Resident→Ready; bridge called exactly on request and
retry, never in poll spins; bounded retries then Expired; fatal ends
Failed; unconfirmed residents expire; duplicates/unknown/cancel rules;
terminal reaping.

Fixes: missing thiserror dep; &self/&mut borrow in transition guard
(solved with an associated fn + sed-switched call sites, verified by
build). 16/16 green, clippy 0, fmt clean. Required docs
docs/ASSET_MOUNTING.md + docs/ASSET_RESIDENCY.md added. Workspace
regression: 784 passed / 0 failed.

## 2026-09-16 — ald-streaming: orchestration to Ready

Joins download/cache/mount/residency with explicit game handoffs:
unified states (active downloads report before queued mounts —
precedence fixed after a test exposed the shadow), join gate over
required sets, byte progress, lane scheduling, corrupt-byte finalize
rejection, dep-failure and residency-failure surfacing.

Fixes: assert_eq against `Err(Variant(_))` (pinned exact errors),
moved test helper value, dead struct field + `let _ = job` + clone
lints removed. 10/10 green, clippy 0, fmt clean. Required doc
docs/ASSET_STREAMING.md added. Workspace regression: 794 passed /
0 failed.

## 2026-09-16 — ald-buckets: bucket API over dimensions

Bucket↔dimension 1:1 mapping; player default 0; change hooks with
old/new (same-bucket re-assign silent); entity bucket set/get with dead
ids refused; lockdown grants (bucket-scoped, revoke reports); density
policies synced into `PopulationManager` dimensions and proven by spawn
decisions; bucket/dimension agreement proven in one composition test.

Fixes: ald-ecs now re-exports `EntityId` (was private import blocking
downstream use); test mut; clippy type-complexity alias. 8/8 green,
clippy 0, fmt clean. Workspace regression: 802 passed / 0 failed.

## 2026-09-16 — ald-nui: NUI groundwork (no browser claimed)

`aldnui://` origin parsing (strict names, traversal/NUL/backslash
rejection); bounded message queue with counted oldest-drops; callback
registry with owner checks and exactly-once responses; focus tracking
that refuses focus without an open page and clears on close; CSP builder
deny-by-default with explicit WASM/origin opens.

One clippy derivable-Default fixed by hand. Required doc docs/NUI.md
added. 6/6 green, clippy 0, fmt clean. Workspace regression: 808 passed
/ 0 failed.

## 2026-09-16 — ald-input: canonical keys + conflict reports

Strict case-insensitive catalog (keyboard/mouse, Aldivine-defined —
scancode mapping stays at the bridge); per-resource press bindings with
unbind semantics; F8 reserved unless explicitly allowed; conflicts
reported with claimants in registration order (never silently resolved);
press types partition matching.

6/6 green, clippy 0, fmt clean. Workspace regression: 814 passed /
0 failed.

## 2026-09-16 — ald-dui: runtime-texture groundwork (no renderer claimed)

Texture lifecycle (strict names, bounded dims, dup/destroy rules);
pixel-upload shape validation (`w*h*4` exact); scheme-allowlist
navigation (`file://`/`about:`/unknown refused, case-insensitive
schemes); mouse clamping into texture space; bounded page routing.

Clippy fixes by hand (manual contains, useless vecs). Required doc
docs/DUI.md added. 7/7 green, clippy 0, fmt clean. Workspace regression:
821 passed / 0 failed.

## 2026-09-16 — astryn join driver: ordered client join

`JoinDriver` on virtual ticks: Handshake→Deferrals→Queued→Downloading→
Residency→Ready. Reject/kick land Failed from any phase; out-of-order
messages rejected with phase unchanged; terminal states stick with late
events reported; per-phase stall budgets with activity reset; download
fractions for UI; cancel semantics; zero-budget clamping.

One clippy unused-variable fixed (`total: ..`). Crate 10/10 (2 lifecycle
+ 8 join), astryn-lib clippy clean, fmt clean. Workspace regression:
829 passed / 0 failed.

## 2026-09-16 — ald-voice: channel model + routing (no audio claimed)

Proximity/radio/direct channels with strict ids and bounded membership
(full refuses, re-join idempotent); validated radio frequencies;
proximity tiers by distance; bounded metadata envelopes; mutes gate
transmission, deafens filter fan-out, speaker excluded, unknown channels
fan out to nobody.

Fixes: `Ok` vs `Some` typo in members(); missing max_members args in
radio tests. 7/7 green, clippy 0, fmt clean. Workspace regression: 836
passed / 0 failed (829 + 7 voice).

## 2026-09-16 — foundation: toolchain pin, audit gate, policy docs

rust-toolchain.toml pins 1.98.1 (rustup sync verified working, same
version as validated env). Core isolation verified for real via
`cargo tree`: ald-server closure (126 crates) contains zero
libloading/mlua/rquickjs/bindgen/clang/openssl/cmake/v8/boa/cef/tauri;
only expected OS shims (windows-sys via tokio). Boundaries mapped:
rquickjs→ald-script-js, mlua→ald-script-lua, libloading→
ald-native-extension, node/dotnet halves pure Rust.

`ald dependencies audit-native` implemented as a real CI gate (denylist
× tree parsing, exit 0/1/2) with 3 unit + 1 live e2e test — e2e runs the
built binary in the workspace root and asserts PASS. One clippy
trim-split fix. CLI 11/11, clippy 0, fmt clean.

Docs: NATIVE_DEPENDENCY_POLICY (verified table + boundary-adding
procedure), RUST_FIRST_ARCHITECTURE, BUILD_PROFILES (native real today,
features declared only when wired — none declared yet), WINDOWS_BUILD
(env + honest failure list). Workspace regression: 840 passed / 0 failed.

## 2026-09-16 — PHASE 39: base resources (spawn, chat, session, loading, commands, help, devtools)

Created the seven canonical base resources in `base-resources/`:
1. `spawn`: spawn point storage, client placement requests.
2. `chat`: size-bounded message sanitization, broadcast relay.
3. `session`: player connect/disconnect tracking and count broadcasts.
4. `loading`: client progress tracking and session handoff.
5. `commands`: command registration and dispatch router.
6. `help`: command registry introspection and help message responder.
7. `devtools`: admin coordinates teleport and server stat reporting.

Each resource contains `ald_manifest.toml`, sandboxed `server/init.lua` and
`client/init.lua`. Added e2e test `base_resources_validate_successfully` in
`cli/ald/tests/cli_e2e.rs` validating each manifest via `ald resource validate`.
CLI tests: 12 passed (7 unit, 5 e2e), clippy 0 warnings, fmt clean.
Workspace regression: 841 passed / 0 failed.

## 2026-09-16 — PHASE 46: ald-package-scanner (Registry package scanner)

Created `crates/ald-package-scanner` (10/10 tests):
- In-memory `ReputationAuthority` tracking publisher status (Trusted,
  Verified, Standard, Suspicious, Blacklisted), revoked package IDs,
  and revoked public signing keys.
- Static file path scanner: flags native binary extensions (.dll, .so,
  .dylib, .exe, .bin, .elf) as Critical when native extensions are disallowed,
  or Low when allowed by configuration.
- Static file content scanner: detects dangerous breakout calls (`os.execute`,
  `io.popen`, `package.loadlib`, `child_process`, `setfenv`, `loadstring(`)
  with weighted scoring; detects long continuous base64/hex obfuscation runs.
- Scoring & Verdict engine: computes risk score 0..100 with publisher discounts,
  decides Approved, FlaggedForReview, Quarantined, or Rejected.
- Fixed 2 clippy warnings in `ald-package` (`is_multiple_of` and unused mut).
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 851 passed / 0 failed.

## 2026-09-16 — PHASE 47: client/simulation-client (AstraSim headless simulation client)

Created `client/simulation-client` (`astrasim`) (10/10 tests):
- Deterministic 64-bit `DeterministicRng` (XorShift) ensuring identical seeds
  yield identical states, coordinates, and metrics across runs.
- `SimulatedClient` state machine: Disconnected -> Handshake -> InQueue ->
  ResourceNegotiation -> InWorld -> Reconnecting.
- Queue drain logic: FIFO promotion into resource negotiation.
- Deterministic position updates (2D/3D walk with heading).
- Controlled packet loss injection with sent/received/dropped event accounting.
- Controlled disconnect injection with automatic retry ticks and reconnection.
- Dimension interest transitions across multi-dimension scenarios.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 861 passed / 0 failed.

## 2026-09-16 — PHASE 55: ald-backup (Backups, DR, DB migration safety, aldivine.lock)

Created `crates/ald-backup` (10/10 tests):
- Full and Incremental backup formats with parent tracking.
- Per-entry and manifest SHA-256 integrity calculation and pre-restore validation.
- Target DB migration version checks: prevents restores onto divergent or newer schema versions unless explicit operator migration flag provided.
- `aldivine.lock` serialization, deserialization, and package integrity verification.
- Documented operational procedures and RPO/RTO targets in `docs/BACKUP_RECOVERY.md`.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 871 passed / 0 failed.

## 2026-09-16 — PHASE 57: ald-upgrade-planner (Upgrade compatibility planner)

Created `crates/ald-upgrade-planner` (10/10 tests):
- Topology comparison across NovaGate, Astryn, Server Runtime, AstraNet,
  Framework, Aegis, packages, and DB schema.
- Protocol break analysis: protocol bumps require client updates.
- Client update requirement evaluator: identifies mandatory vs optional upgrades.
- Database schema migration step calculation & destructive backwards migration rejection.
- Rollback limits: Safe, Conditional, Impossible.
- Restart severity classification: None, HotReload, GracefulServerRestart, FullStackRestart.
- Created `docs/UPGRADE_PLANNER.md` operational guide.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 881 passed / 0 failed.

## 2026-09-16 — PHASE 60: ald-symbol-service (Crash Dumps, Symbol Service, Safe Mode)

Created `crates/ald-symbol-service` (10/10 tests):
- Raw crash dump representation across WindowsMinidump, LinuxCoreDump, and GenericPanic.
- Symbol tables mapping address to symbol name, source file, line number.
- Stack symbolication resolving raw execution addresses into readable frames.
- Deterministic 16-hex crash fingerprint generation based on fault frames and resource context.
- `SafeModeDetector`: sliding-window crash-loop detection, culprit resource identification,
  automatic Safe Mode activation, and Aegis operator recovery guidance.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 891 passed / 0 failed.

## 2026-09-16 — PHASE 61: ald-replay (Diagnostic Replay & Resource Debugger)

Created `crates/ald-replay` (10/10 tests):
- Deterministic session timeline recording (`ReplayRecording`) capturing events,
  native calls, state bag mutations, entity spawns, and resource lifecycles.
- Interactive timeline debugger (`ReplayDebugger`): single-step forward,
  seek forward and backward, conditional tick breakpoints, event name breakpoints.
- Dynamic reconstruction and querying of active live entities at any point in the timeline.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 901 passed / 0 failed.

## 2026-09-16 — PHASE 58: cli/ald (ald report create diagnostic bundle)

Extended `cli/ald` with `ald report create`:
- Gathers environment, server configuration, and resource manifests.
- Implements strict multi-tier secret sanitization for passwords, tokens,
  API keys, private keys, database URLs, and hardware identifiers.
- Fixed infinite loop in database URL prefix search during sanitization.
- Added e2e test in `cli/ald/tests/cli_e2e.rs` verifying valid JSON bundle output.
- CLI tests: 17 passed (11 unit, 6 e2e), clippy 0 warnings, fmt clean.
- Workspace regression: 906 passed / 0 failed.

## 2026-09-16 — PHASE 48: agent/aegis-node (Aegis Node Agent supervisor)

Created `agent/aegis-node` (10/10 tests):
- Server process supervisor isolating process ownership from web browsers.
- Lifecycle state machine: Stopped -> Starting -> Running -> Stopping -> Crashed -> Quarantined.
- Watchdog heartbeat poller with automatic restart on timeout.
- Crash-loop detector with retry caps and automatic quarantine.
- In-memory bounded log ring buffer preserving recent stdout/stderr.
- Build deployment updater and rollback restoring last known-good build.
- Multi-instance supervisor isolation.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 916 passed / 0 failed.

## 2026-09-16 — PHASE 59: ald-security (Anti-Cheat, SSRF, DDoS, Client Integrity)

Extended `crates/ald-security` (12/12 tests):
- Server-authoritative `AntiCheatDetector` evaluating 3D physical speed bounds,
  teleport spikes, world boundary validation (-4500..5500 X, -4500..8500 Y, -200..3000 Z),
  and client-side health increase verification.
- `SsrfFilter` validating outbound webhook/HTTP URLs, blocking loopback (127/8),
  RFC 1918 private subnets (10/8, 172.16/12, 192.168/16), and cloud metadata (169.254.169.254).
- `DdosRateLimiter` implementing token buckets with burst capacity and time-based refill.
- `ClientIntegrityVerifier` registering allowed builds and verifying SHA-256 binary digests.
- Preserved existing log secret `Redactor` with 100% test coverage.
- 12 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 925 passed / 0 failed.

## 2026-09-16 — PHASE 65: ald-updater (Release Infrastructure, Channels, Signing, SBOM, Rollback)

Created `crates/ald-updater` (10/10 tests):
- Release channel hierarchy: Stable, Recommended, Beta, Canary, Nightly, Development.
- Ed25519 signature verification over canonical signed payload strings.
- Immutable release catalog refusing duplicate / overwrite attempts.
- Software Bill of Materials (`SbomDocument`) component tracking and integrity checksums.
- `UpdateChecker` generating actionable update plans comparing current vs channel latest.
- Rollback target resolution traversing previous artifact releases.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 935 passed / 0 failed.

## 2026-09-16 — PHASE 74: ald-telemetry & docs/PRIVACY.md (Localization, Accessibility, Telemetry Consent)

Extended `crates/ald-telemetry` (9/9 tests):
- `TelemetryConsent` with default-deny semantics and category-level gating
  (PerformanceMetrics, CrashReporting, UsageAnalytics, NetworkDiagnostics).
- `AccessibilitySettings` covering high-contrast, text scaling, reduced motion,
  screen reader hints, and colorblindness adaptations.
- `LocalizationCatalog` supporting locale-specific templates, fallback to default,
  and `{var}` string interpolation.
- Updated `docs/PRIVACY.md` detailing telemetry consent boundaries and local settings guarantees.
- 9 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 941 passed / 0 failed.

## 2026-09-16 — PHASE 66: ald-ha (Central Service HA & Offline Fault Tolerance)

Created `crates/ald-ha` (10/10 tests):
- State-machine `CircuitBreaker` (Closed -> Open -> HalfOpen) preventing cascading RPC/HTTP failures.
- Prioritized `FailoverPool` rotating between multiple central service replicas.
- `OfflineGracePolicy` supporting uninterrupted local gameplay and cached entitlement validation during central network partitions.
- Epoch-based monotonic `LeaderCoordinator` avoiding split-brain collisions in multi-node edge deployments.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 951 passed / 0 failed.

## 2026-09-16 — PHASE 67: ald-sharding (Sharding Groundwork, Player Handoff, Distributed Ownership)

Created `crates/ald-sharding` (10/10 tests):
- Spatial world partitioning across named shard bounding regions.
- Atomic two-phase player handoff protocol (`Initiated` -> `Prepared` -> `Committed` / `Aborted`)
  ensuring no player drops or duplicate entity ownership during boundary traversal.
- Distributed entity `OwnershipLease` with monotonic epoch fencing, lease renewal,
  conflict rejection, and expiration reclamation.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 961 passed / 0 failed.

## 2026-09-16 — PHASE 68: ald-compat-lab (Compatibility Lab & Exact Certification)

Created `crates/ald-compat-lab` (10/10 tests):
- `CertifiedSupportMatrix` with 21 explicit gates evaluating GTA builds, script runtimes,
  datafiles, assets, databases, framework compatibility, streaming chaos, and native audit.
- Strict single source of truth rule: `is_100_percent_certified()` returns true ONLY if
  100% of required gates are `GateResult::Pass`. Any Fail or Skipped gate blocks certification.
- Blocker reporting: `blocking_gates()` enumerates exact gating hurdles.
- Test corpus harness (`CompatibilityLab`) running synthetic fixtures and real-world OSS resources.
- 10 tests green, clippy 0 warnings, fmt clean.
- Workspace regression: 971 passed / 0 failed.

## 2026-09-16 — PHASE 75 & 76: Production Validation, Documentation Finalization & RC1

- Audited and authored all 53 required master specification documents across `docs/`
  and `docs/compatibility/`:
  - `docs/PROTOCOL_COMPATIBILITY.md`, `docs/CRYPTOGRAPHY.md`, `docs/SERVER_CFG.md`,
    `docs/SERVER_CONFIGURATION.md`, `docs/AEGIS.md`, `docs/AEGIS_SETUP.md`,
    `docs/AEGIS_DESIGN_SYSTEM.md`, `docs/ASTRYN_DEVELOPER_CONSOLE.md`,
    `docs/CFXLUA_COMPATIBILITY.md`, `docs/ASSET_RECOVERY.md`, `docs/EDGE.md`,
    `docs/SECRETS.md`, `docs/SUPPORT_MATRIX.md`, `docs/PERFORMANCE_SLO.md`,
    `docs/compatibility/CITIZEN_API_MATRIX.md`, `docs/compatibility/CORE_EVENT_MATRIX.md`,
    `docs/compatibility/GAME_EVENT_MATRIX.md`, `docs/compatibility/NATIVE_MATRIX.md`.
- Completed security and cryptographic review: strictly standard primitives (Ed25519,
  Argon2id, AES-256, SHA-256), no custom crypto invented, server-authoritative physics,
  SSRF filters, and data-minimization privacy rules verified.
- Audited native boundaries: `ald dependencies audit-native` confirmed 0 unexpected C/C++
  dependencies in `ald-server` core.
- Toolchain: pinned to Rust 1.98.1 in `rust-toolchain.toml`.
- Clippy: 0 warnings across workspace (`cargo clippy --workspace --all-targets`).
- Formatting: `cargo fmt --all -- --check` clean.
- Final workspace regression: 971 passed / 0 failed. Project Aldivine v0.1.0-RC1 achieved.

## 2026-09-16 — Vertical remediation: server admission, real handshake, Lua execution, Aegis API, e2e

- Added shared handshake wire (`ald-protocol::handshake`): tagged
  HandshakeMessage + HandshakeState + reject codes + ResourceEntry; 17 tests.
- Server: new `admission` (Hello/Challenge/Auth/Identity+policy/Deferral/Queue/
  Manifest/Ready state machine, 9 tests), `config_loader` (server.cfg
  first-class: --config/server.cfg/server.toml/default, 4 tests), real channel
  dispatch in `network_loop` (Auth drives admission, bad packets counted not
  fatal), SHA-256 resource manifest published to admission, framework
  PlayerService join on ServerReady, Aegis Axum API on 127.0.0.1:40120 (5 tests,
  public bind refused). `spawn` Lua server scripts execute for real in the
  sandboxed runtime; missing resources fail honestly.
- Astryn: real UDP handshake walk; Running only on ServerReady world_join.
  No-server never reaches Running (test), bad address fails without panic.
- `server/ald-server/tests/vertical_e2e.rs`: loopback join reaches Running and
  registers framework player (online >= 1). Server 29 lib tests green.
- Docs: new `docs/VERTICAL_INTEGRATION.md`; honesty pass on
  IMPLEMENTATION_STATUS (Astryn split, queue/deferrals wired, assets
  recognition-only) and SUPPORT_MATRIX (evidence tiers, synthetic vs
  integration); `server.cfg.example`; `.github/workflows/ci.yml`.

## 2026-09-16 — Vertical hardening: states, gate, transfer, Aegis product

- Resource lifecycle now 13 states (ald-resource, 7 tests); supervisor walks
  VALIDATING -> DEPENDENCY_RESOLUTION -> SCRIPT_HOST_STARTING ->
  MIGRATIONS_READY -> HEALTH_CHECKING -> HEALTHY; HEALTHY requires manifest,
  deps, scripts, migrations, health gate. Quarantine action added.
- Ingress gate in dispatch: per-peer token bucket (300/150s), client channel
  allowlist (+ResourceTransfer), Auth-only unknown peers, rejected dropped
  (state.rs, 3 tests).
- server.cfg `exec` includes resolved from disk with cycle/traversal guards
  (config_loader, 2 tests).
- Welcome event `ald:server:welcome` on EventReliable after ServerReady;
  client captures it (e2e asserted).
- Download transport v1: ald-protocol transfer codec (5 tests),
  server chunk serve with containment (3 tests), Astryn fetch with stall
  bound + SHA-256 + cache commit; e2e downloads spawn manifest byte-identical.
- Aegis: setup/status + setup/complete provisioning (3 tests), React UI
  aegis/web (Vite+TS+Tailwind sky) with setup wizard + live dashboard,
  `npm run build` green; CI web job added.
