# Data File Matrix

Recognition status per GTA DataFile entry. Only filenames stated in the
platform spec's asset graphs are seeded; other graphs register their
entries with their phases plus Compatibility Lab evidence.

| File | Category | Recognition | Content support |
|------|----------|-------------|-----------------|
| vehicles.meta | Vehicle | SpecSeed | PLANNED (vehicle graph) |
| handling.meta | Handling | SpecSeed | PLANNED (vehicle graph) |
| carcols.meta | Appearance | SpecSeed | PLANNED (vehicle graph) |
| carvariations.meta | Variation | SpecSeed | PLANNED (vehicle graph) |
| vehiclelayouts.meta | Layout | SpecSeed | PLANNED (vehicle graph) |

Source: `ald-datafiles` (`DataFileRegistry::with_spec_seed`, 6 tests).
Ped, clothing, MLO, audio, weapon, population, and other DataFiles are
PLANNED with their graph phases — not seeded from memory.
