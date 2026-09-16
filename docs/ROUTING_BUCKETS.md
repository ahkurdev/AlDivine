# Routing Buckets / Dimensions

Native form: dimensions (`ald-ecs::dimensions`). Compatibility form
(player/entity bucket API) binds in the script-host phases; the isolation
semantics below are what it must preserve.

## Rules

- Every live entity is in exactly one dimension; unassigned reads as
  `DEFAULT` (0). Destroyed ids read as nothing — slot reuse starts clean.
- Visibility is strict same-plane. Interest sets are computed spatially
  (`spatial::replication_set`) then filtered to the observer's plane; the
  composition is proven by test, not assumed.
- Assignment to dead ids is refused (no ghost claims). Destroy paths call
  `DimensionMap::remove`; `prune` is the backstop, reporting removals.
- Ownership transfer (`ald-ecs::ownership`) is compare-and-swap: server
  delegates, owners release to the server, lateral player→player or
  resource→resource handoffs are refused (`MustReleaseFirst`) — migration
  is release + re-acquire, server-mediated. Stale ids are `NotFound`.

## Status

Ownership (9 tests) + dimensions (6 tests) IMPLEMENTED in `ald-ecs`
(26/26 with pre-existing store/spatial tests). Population density policy
and the Citizen bucket-compat surface are later work.
