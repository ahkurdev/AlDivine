# Asset Residency

Residency handshake (`ald-residency`): Mounted/Registered is not ready.

## State machine

Requested -> Waiting -> Resident -> Ready. Terminal: Ready, Failed,
Expired, Cancelled. Only Ready means usable — mount != ready, enforced.

## Rules

- Virtual tick clock: deadlines, timeouts, and retries are deterministic.
- The bridge (`GameCommit`) is called exactly on request and retry — never
  in a poll spin. Polls on Waiting only watch the deadline.
- Retries bounded by `max_retries`; exhaustion ends Expired. Fatal bridge
  answers end Failed. Unconfirmed residents expire.
- Duplicate request ids refused; terminal requests report without touching
  the bridge; `reap_terminal` keeps long-lived trackers clean.

## Status

IMPLEMENTED (8 tests with a scripted bridge). The real `GameCommit`
arrives with the game bridge (BLOCKED_EXTERNAL: no GTA on dev box).
