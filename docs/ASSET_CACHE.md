# Asset Cache

Content-addressed object store (`ald-cache`): `objects/ab/cdef…` by sha256,
`tmp/` staging, atomic verified commits, integrity repair, quota/LRU with
mounted-content protection.

## Rules

- Same bytes land once (dedup is structural).
- Staged bytes are never served; `commit()` re-hashes and only then
  renames atomically. Mismatches remove the temp and error — partial files
  are never valid.
- `verify_all()` reports health; `repair()` deletes corrupt objects and
  reports removals. Repair never fabricates bytes; callers re-fetch via
  the download engine.
- `evict_to_fit()` deletes least-recently-used first until within quota;
  protected (mounted) hashes are exempt. Recency tracks in memory and
  rebuilds from mtimes on open.
- Stray files the cache never wrote are ignored, never served.

## Status

IMPLEMENTED (10 tests over real temp-dir files). Mount integration
(protecting live mounts, cache-aware fetch) arrives with the mount phase.
