# Asset Format Matrix

Recognition status per GTA asset format. Recognition means routing
(extension → stream class); content support (parse, verify, mount, stream)
is certified separately per format with Compatibility Lab evidence.

| Extension | Stream class | Recognition | Content support |
|-----------|--------------|-------------|-----------------|
| .ytd | Texture | SpecSeed | PLANNED |
| .yft | Geometry | SpecSeed | PLANNED |
| .ydd | Geometry | SpecSeed | PLANNED |
| .ydr | Geometry | SpecSeed | PLANNED |
| .ybn | Collision | SpecSeed | PLANNED |
| .ymap | Map | SpecSeed | PLANNED |
| .ytyp | Map | SpecSeed | PLANNED |
| .ycd | Animation | SpecSeed | PLANNED |
| .awc | Audio | SpecSeed | PLANNED |
| .rel | Data | SpecSeed | PLANNED |
| .ymt | Metadata | SpecSeed | PLANNED |

Source: `ald-asset-registry` (`FormatRegistry::with_spec_seed`, 6 tests).
No row is content-certified: certification requires real files plus lab
runs (streaming/cache phases). Recognition != support.
