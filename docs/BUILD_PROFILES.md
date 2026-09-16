# Build Profiles

Modular deployment shapes. Profiles are additive: start native, add only
the compatibility runtimes a server's resources require.

## Current status (verified 2026-09-16)

The native profile is real today: `ald-server` builds and runs with zero
compat-runtimes in its closure (see `docs/NATIVE_DEPENDENCY_POLICY.md`).
Compat runtimes exist as independent workspace crates and are not linked
into the server binary — optional loading is structural, not configured.

## Target profile map

| Profile | Contents |
|---------|----------|
| native | ald-server as built: config, network, resources, scheduler, console |
| citizen-compat | + ald-cfxlua-compat, state bags, buckets, Citizen event surface |
| node16-compat | + Node 16 host profile (engine boundary) |
| node22-compat | + Node 22 host profile (engine boundary) |
| dotnet-compat | + CLR host profile (engine boundary) |
| voice | + ald-voice routing (metadata; audio transport separate) |
| edge | + ald-edge gateway |
| diagnostics | + aldreport bundle, symbol service hooks |
| full | all of the above |

## Wiring rule

Cargo features (`native`, `citizen-compat`, `node16-compat`,
`node22-compat`, `dotnet-compat`, `voice`, `edge`, `diagnostics`, `full`)
land on the server binary when optional runtime loading lands. Until
then, profiles are selected by which crates a deployment builds — no
feature flags are declared that wire to nothing.
