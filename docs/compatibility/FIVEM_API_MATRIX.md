# FiveM API Compatibility Matrix

Aldivine Compatibility Layer (ACL) coverage for the API patterns legacy
resources call. Every entry is backed by an adapter implementation and a
test; status is generated from the adapters in `crates/acl-*`, not estimated.

Statuses:

- **SUPPORTED** — behaviorally equivalent, routed through the authoritative
  Aldivine service.
- **PARTIAL** — works for the common case; documented divergences remain
  (usually: the legacy call could mutate server state from any resource, so
  it is accepted only from server-side context).
- **NOT_SUPPORTED** — no safe server-authoritative equivalent. Port the
  resource to the native API.
- **BLOCKED** — requires an unavailable external interface.

## ESX (`crates/acl-esx`)

| Legacy call | Status | Notes |
|---|---|---|
| `ESX.GetPlayerFromId(source)` | SUPPORTED | Returns a projection over `PlayerService`; money in cents |
| `ESX.GetPlayerFromIdentifier` | SUPPORTED | Identifier mapped through `ald-identity` |
| `xPlayer.getMoney()` / `getAccount` | SUPPORTED | Read-only sum of cash + bank |
| `xPlayer.addMoney` / `removeMoney` | PARTIAL | Server-context only; never client-callable |
| `xPlayer.getInventoryItem` | SUPPORTED | Read-only projection over `InventoryService` |
| `xPlayer.addInventoryItem` | PARTIAL | Server-side grant; weight/stack validated |
| `xPlayer.removeInventoryItem` | PARTIAL | Server-side take; fails on insufficient |
| `xPlayer.getJob()` | SUPPORTED | Projects `JobService` employment |
| `esx:registerUsableItem` | NOT_SUPPORTED | Item-use callbacks are a native Aldivine hook, not a global registry |
| `ESX.RegisterServerCallback` | NOT_SUPPORTED | Use the typed native RPC system |

## QBCore (`crates/acl-qbcore`)

| Legacy call | Status | Notes |
|---|---|---|
| `QBCore.Functions.GetPlayer(source)` | SUPPORTED | Session-source lookup |
| `QBCore.Functions.GetPlayerByCitizenId` | SUPPORTED | citizenid map |
| `PlayerData.money` | SUPPORTED | Read-only |
| `QBCore.Functions.GetJob` | SUPPORTED | |
| `QBCore.Functions.AddItem` | PARTIAL | Server-side; validated grant |
| `QBCore.Functions.RemoveItem` | PARTIAL | Server-side; validated take |
| `QBCore.Player.UpdatePlayerData` | PARTIAL | Coalesced; not a per-call network broadcast |

## Qbox (`crates/acl-qbox`)

Qbox descends from QBCore; the adapter delegates player lookup to the QBCore
implementation and only overrides the flavor and the capability namespace.

| Legacy call | Status |
|---|---|
| `GetPlayerByCitizenId` | SUPPORTED |
| `PlayerData.money` | SUPPORTED |
| `GetJob` | SUPPORTED |
| `AddItem` / `RemoveItem` | PARTIAL |
| `UpdatePlayerData` | PARTIAL |
| `QBCore.Functions.*` (any) | NOT_SUPPORTED — Qbox dropped this namespace |

## Categories not yet covered

These are PLANNED, not implemented:

- event registration / server-client event translation
- callbacks (`RegisterNetEvent`-style)
- exports (resource-to-resource)
- commands
- resource manifest translation (`fxmanifest.lua` -> `ald_manifest.toml`)
- timers / threads / coroutines
- HTTP helpers
- database adapters (`oxmysql` / `mysql-async` emulation)
- state bags

## Verification

Coverage is asserted by unit tests in each adapter crate:

```
cargo test -p acl-esx -p acl-qbcore -p acl-qbox
```

The claim "FiveM scripts work" is NOT made. Compatibility is per-API and
per-flavor, as the tables above state.
