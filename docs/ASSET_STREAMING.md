# Asset Streaming

Orchestration (`ald-streaming`) across download, cache, mount, and
residency — driven on caller ticks and bytes, no threads or hidden timers.

## Flow per asset

add_asset -> receive_chunk* -> finalize_download (cache commit)
-> mount_verify -> mount_check_deps -> mount_begin
-> complete_mount (caller: game commit done)
-> mount_register -> residency_request -> residency_poll* -> Ready

## Rules

- Unknown formats refused at `add_asset`; duplicates refused.
- Unified `asset_state`: Downloading / Stalled / Caching / Mounting /
  Residency / Ready / Failed with reasons. Active downloads report
  before queued mounts; failures surface with reasons.
- `overall_progress` accounts bytes across assets; `join_ready` gates
  join on a required set (unknown ids are not ready, never panic).
- Game touchpoints stay explicit: `complete_mount` and the residency
  bridge are caller-confirmed, never assumed.

## Status

IMPLEMENTED (10 tests over real cache files with a scripted bridge).
Transport, CDN auth/failover, FPS-aware worker scaling later.
