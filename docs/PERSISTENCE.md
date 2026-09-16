# Persistence

Crash-safe world persistence: versioned snapshots, append-only journal,
atomic checkpoints, and recovery that distinguishes torn tails from
corruption.

## Pieces

- `ald-snapshot`: `Snapshot { schema_version, tick, entries }` with a
  checksummed binary framing (`ALDSNAP1`). Decode rejects wrong magic,
  unsupported schema, truncation (cut at any point), trailing garbage,
  checksum mismatch, and oversized entries — each with its own error.
- `ald-persistence`: `Journal` in one directory (`journal.log` +
  `checkpoint.bin`). Sequences start at 1 and never reuse across
  restarts. `checkpoint()` writes atomically (tmp + rename) then
  truncates the journal, compacting dead history. `recover()` returns the
  validated checkpoint plus strictly-later journal records.

## Crash rules

- Torn journal tail (crash mid-append): dropped **and reported**
  (`torn_tail_dropped`), recovery succeeds.
- Bad checksum mid-journal: `Corrupt(offset)` — only the final record may
  be torn; anything else fails closed.
- Torn checkpoint: ignored (`checkpoint_ignored`), replay from scratch.
- Full-length checkpoint with bad checksum or trailing garbage:
  `BadCheckpoint` — a bad base must not poison replay.
- Failed appends consume no sequence; write path bounds keys (256 B) and
  values (1 MiB).

## Status

Both crates IMPLEMENTED (9 + 9 tests, all against real files in temp
dirs — crash conditions simulated by truncating/flipping actual bytes).
Binding to live server state (which structures get snapshotted, when
checkpoints fire) is runtime-integration work.
