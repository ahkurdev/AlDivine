# State Bags

Replicated key/value stores with ownership, versions, and change handlers.

## Model (`ald-state`)

- One `StateBag` per entity, player, or the global scope, held in a
  `Registry`. Values are typed (`Nil/Bool/Int/Float/Str/Bytes/Array/Map`),
  never JSON-forced.
- Every mutation bumps a per-bag sequence; handlers observe
  `{bag, key, old, new, seq}` in subscription order.
- Write policies: `Open` (anyone) or `OwnerOnly` (recorded owner only —
  no owner recorded means nobody may write).
- Bounds: keys non-empty/NUL-free/`MAX_KEY_LEN`; values size-, breadth-,
  and depth-bounded; per-key replication flags exclude server-local
  scratch from `snapshot_replicated()`.
- Handlers are observers by construction (they receive only `&ChangeEvent`
  and cannot write back into the observed bag). Cross-bag reactions belong
  to the orchestration layer.

## Compatibility (`ald-statebag-compat`)

- Scope mapping: `GlobalState` → `global`, `Entity(id).state` →
  `entity:<id>`, `Player(id).state` → `player:<id>`, `LocalPlayer.state` →
  `localplayer`. Names round-trip exactly; malformed names are errors.
- Key rule: non-empty, NUL-free, fits the tighter core bound.
- Strict mode (`StrictBags`): scoped bags created with an owner, writes
  gated on it, explicit `transfer` for handoffs (re-create with a different
  owner is refused; wrong-`from` transfer is refused). Nothing is created
  on the write path.

## Status

`ald-state`: IMPLEMENTED (10 tests). `ald-statebag-compat`: IMPLEMENTED
(8 tests). Replication transport binding (feeding `snapshot_replicated`
into AstraNet STATE_SYNC with per-client interest) is later work.
