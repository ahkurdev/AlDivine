# Game Event Matrix

This document tracks low-level GTA V gameplay events intercepted by the Astryn Game Event Router (`crates/ald-game-events`).

| Game Event | Parameters / Decoded Payload | Status | Handling |
|---|---|---|---|
| `CEventNetworkEntityDamage` | victim, attacker, weapon_hash, is_fatal | **IMPLEMENTED** | Deduplicated & routed to server for health damage sync |
| `CEventNetworkPlayerDeath` | victim_player_id, killer_id, death_reason | **IMPLEMENTED** | Dispatched to spawn/session managers |
| `CEventNetworkVehicleUndriveable` | vehicle_id, engine_health, is_destroyed | **IMPLEMENTED** | Synced to vehicle persistence and ECS state |
| `CEventNetworkPlayerCollectedPickup`| player_id, pickup_hash, amount | **IMPLEMENTED** | Validated server-side against inventory bounds |
| `CEventNetworkPedEnteredVehicle` | ped_id, vehicle_id, seat_index | **IMPLEMENTED** | Updates entity attachment and ownership hierarchy |
| `CEventNetworkPedLeftVehicle` | ped_id, vehicle_id, seat_index | **IMPLEMENTED** | Clears seat occupancy |
