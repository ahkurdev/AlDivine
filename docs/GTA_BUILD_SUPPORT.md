# GTA Build Support

How Aldivine decides whether a connecting client's game build may join,
and what "supported" means for a build.

## Principle

A build number this server has never seen is **unsafe until proven
otherwise**: unknown builds are refused by default. Accept-and-hope is
how incompatible clients corrupt world state and how unknown executables
dodge native-version checks.

## Assessment order (`ald-game-builds`)

1. Edition supported here? (`EditionProfile`, see `docs/GTA_EDITIONS.md`)
2. Exact build known for that edition?
3. Build blocked (known-bad: crash, exploit, broken natives)?
4. Client Astryn at or above the build's `min_astryn`?

Outcomes: `Supported`, `RequiresAstrynUpdate`, `Blocked`, `UnknownBuild`,
`UnsupportedEdition`. Only `Supported` is admittable. Unparsable client
versions fail closed (treated as too old).

## Certified corpus: none yet

The registry ships **empty by design**. Entries become certified only
through the Compatibility Lab against real installs, with evidence per
entry (lab run + game build + Aldivine version). The crate's test suite
uses synthetic numbers (90000+) that cannot collide with real builds —
green tests prove registry logic, never coverage of any real game version.

Do not hand-populate the registry from memory or forum posts. An entry
without lab evidence is uncertified data wearing a certified uniform.

## Emergency compatibility

When Rockstar ships an overnight update, clients arrive on unknown builds.
Operator policy per edition (`EmergencyPin`):

- Default: refuse with a clear message.
- Opt-in: pin to the edition's `last_good_build` profile **with a warning
  the client and Aegis both see**. The fallback build must itself be a
  known registry entry — a pin to an unknown build is refused.
- Pinning never overrides `Blocked` or other known verdicts.

## Status

`ald-game-builds`: PARTIAL (registry, assessment, emergency pin; 13 tests
on synthetic data). Certified corpus: BLOCKED_EXTERNAL — no GTA V install
on this dev box; requires Compatibility Lab runs (phase 67+).
