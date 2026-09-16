# CitizenFX API Compatibility Matrix

This matrix documents the support status of CitizenFX runtime APIs in Project Aldivine.

| API Function | Environment | Status | Implementation / Mapping |
|---|---|---|---|
| `CreateThread` | Client / Server | **IMPLEMENTED** | Sandboxed Tokio task / coroutine scheduler |
| `Wait` | Client / Server | **IMPLEMENTED** | Async sleep integrated with virtual ticks |
| `SetTimeout` | Client / Server | **IMPLEMENTED** | Timer wheel in `ald-scheduler` |
| `RegisterNetEvent` | Client / Server | **IMPLEMENTED** | Registered in event dispatcher |
| `RegisterServerEvent` | Server | **IMPLEMENTED** | Alias to `RegisterNetEvent` |
| `AddEventHandler` | Client / Server | **IMPLEMENTED** | Subscribed to local/network event bus |
| `TriggerEvent` | Client / Server | **IMPLEMENTED** | In-process local event dispatch |
| `TriggerServerEvent` | Client | **IMPLEMENTED** | Serialized over AstraNet reliable event channel |
| `TriggerClientEvent` | Server | **IMPLEMENTED** | Fan-out to target player or broadcast (-1) |
| `exports` | Client / Server | **IMPLEMENTED** | Cross-resource module exported function table |
| `GetPlayerIdentifiers` | Server | **IMPLEMENTED** | Mapped from `ald-identity` |
| `GetPlayerName` | Server | **IMPLEMENTED** | Player identity display name |
| `GetPlayerEndpoint` | Server | **IMPLEMENTED** | Client remote IP (sanitized) |
| `GetPlayerPing` | Server | **IMPLEMENTED** | AstraNet RTT monitor |
| `DropPlayer` | Server | **IMPLEMENTED** | Kick connection with reason |
| `SetRoutingBucket` | Server | **IMPLEMENTED** | Dimension mapping in `ald-buckets` |
| `GetRoutingBucket` | Server | **IMPLEMENTED** | Mapped from `ald-buckets` |
| `Entity(...).state` | Client / Server | **IMPLEMENTED** | State bag replication in `ald-statebag-compat` |
| `GlobalState` | Client / Server | **IMPLEMENTED** | Global state bag with strict ownership |
