# GTA Native Invocation Matrix

This document tracks native invocation layer mappings in the Astryn Native Bridge.

| Native Identifier / Hash | Scope | Status | Notes |
|---|---|---|---|
| `GET_PLAYER_PED` (0x43A66C31C68491C0) | Client | **IMPLEMENTED** | Resolves local player ped handle |
| `SET_ENTITY_COORDS` (0x06843DA7060A026B) | Client | **IMPLEMENTED** | Relocates entity coordinates |
| `GET_ENTITY_COORDS` (0x3FEF770D40960D5A) | Client | **IMPLEMENTED** | Returns vector3 world position |
| `SET_ENTITY_HEADING` (0x8E2530AA8ADA980E) | Client | **IMPLEMENTED** | Updates entity rotation heading |
| `CREATE_VEHICLE` (0xAF35D0D2583051B0) | Client | **IMPLEMENTED** | Spawns vehicle with model hash |
| `DELETE_ENTITY` (0xAE3CBE5BF394C9C9) | Client | **IMPLEMENTED** | Removes entity and cleans local handle |
| `NETWORK_GET_NETWORK_ID_FROM_ENTITY` | Client | **IMPLEMENTED** | Maps local entity handle to network ID |
| `NETWORK_GET_ENTITY_FROM_NETWORK_ID` | Client | **IMPLEMENTED** | Maps network ID to local entity handle |
| `SET_WEATHER_TYPE_PERSIST` (0xED712CA327FA1CDE) | Client | **IMPLEMENTED** | Applies synchronized world weather |
| `NETWORK_OVERRIDE_CLOCK_TIME` | Client | **IMPLEMENTED** | Applies synchronized world time |
