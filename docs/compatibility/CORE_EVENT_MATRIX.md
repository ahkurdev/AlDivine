# Core Event Matrix

This document tracks supported core engine events in Project Aldivine.

| Event Name | Trigger Context | Status | Description |
|---|---|---|---|
| `onResourceStart` | Server / Client | **IMPLEMENTED** | Fired when a resource initializes its scripts |
| `onResourceStop` | Server / Client | **IMPLEMENTED** | Fired before a resource unloads and flushes state |
| `playerConnecting` | Server | **IMPLEMENTED** | Triggered during admission queue / deferrals |
| `playerJoining` | Server | **IMPLEMENTED** | Triggered after deferrals pass, before spawn |
| `playerDropped` | Server | **IMPLEMENTED** | Triggered on disconnect or kick with drop reason |
| `rconCommand` | Server | **IMPLEMENTED** | Triggered on remote console command invocation |
| `populationPedCreating` | Server | **IMPLEMENTED** | Hook in `ald-population` allowing resource cancellation |
| `entityCreated` | Server / Client | **IMPLEMENTED** | Dispatched when an ECS network entity is spawned |
| `entityRemoved` | Server / Client | **IMPLEMENTED** | Dispatched when an ECS network entity is destroyed |
