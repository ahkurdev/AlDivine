# Asset Mounting

Mount pipeline (`ald-mount`): verified files become registered mounts.

## State machine

Queued -> Verifying -> Verified -> DependenciesReady -> MountPending
-> Mounted -> Registered. Terminal: FailedRetryable, FailedFatal,
Cancelled, Registered. Every transition is a named method; illegal jumps
are `BadTransition`; terminal states stick.

## Gate order

1. Queue: filename must be a recognized format — unknown refused at the
   door, never queued.
2. Verify: cache hash verdict. Mismatch is retryable, never fatal.
3. Dependencies: every declared DataFile must resolve. Missing entries
   are retryable with names listed.
4. begin_mount records the mount point; complete_mount and register hand
   off to residency.

## Status

IMPLEMENTED (8 tests). Game-thread commit hookup arrives with the bridge;
this crate ends at Registered.
