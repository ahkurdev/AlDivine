# GTA Editions

Legacy and Enhanced are separate compatibility universes. Aldivine types
the edition once (`ald-game-editions::GameEdition`) and every downstream
system — GameBridge profiles, native tables, asset/DataFile rules, build
resolution — keys off it.

## The two editions

| Edition | Identifier | Meaning |
|---------|------------|---------|
| Legacy | `legacy` | Original PC release line (pre-Gen9). |
| Enhanced | `enhanced` | Current-generation (Gen9) release line. |

## Rules

- Edition is **declared**, never guessed. NovaGate detection reports what
  it found; operator config states what the server expects; the client
  handshake states what it runs. `GameEdition::parse` accepts exactly
  `legacy` / `enhanced` (case-insensitive) and rejects everything else.
- There is no `Unknown` variant. Code that needs "edition not supported
  here" uses `EditionProfile { supported: false, note }`, so the reason
  travels with the refusal to Aegis and the client.
- An edition with no declared profile is unsupported. Absence of data must
  never read as support.

## Status

`ald-game-editions`: IMPLEMENTED (parsing, identifiers, profiles; 4 tests).

Per-edition GameBridge / native / asset profiles are reserved slots in
`EditionProfile`'s consumers and arrive with their own phases (20–22, 27+).
