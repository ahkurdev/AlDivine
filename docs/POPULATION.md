# Population

Server-side ambient budget enforcement (`ald-population`).

## Rules

- Budgets are per dimension: peds and vehicles each have a max and a
  density fraction; the effective cap is `floor(max * density)`.
- Density is a deterministic steady-state fraction, not a dice roll — the
  same request sequence always yields the same decisions. Stochastic
  thinning happens in the game bridge against live world state.
- Unconfigured dimensions deny with `NoPolicy` (fail closed).
- Tightening a policy never retroactively culls; over-cap dimensions deny
  new spawns until releases drain them.
- Releases saturate at zero and report success; double-release is `false`,
  not an underflow.
- Creation hooks (`on_decision`) observe every allow/deny in subscription
  order with the count after the decision.

## Status

IMPLEMENTED (10 tests). Actual spawning, GTA handles, renderer budgets,
and liar enforcement (bridge spawning without asking) belong to the game
bridge and its reconciliation pass.
