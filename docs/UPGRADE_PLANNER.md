# Aldivine Upgrade Compatibility Planner Specification

This document specifies the upgrade compatibility planning subsystem for Project Aldivine.

## 1. Purpose

Before applying major updates, the Upgrade Planner computes dependency and compatibility graphs across the full Aldivine stack:
- **NovaGate Launcher**: Desktop launcher and bootstrap versions
- **Astryn Client**: Native multiplayer runtime version
- **Server Runtime**: `ald-server` core daemon
- **AstraNet**: Wire networking protocol version
- **Aldivine Framework**: Native gameplay framework version
- **Aegis**: Server administration and node agent version
- **Packages**: Installed resource packages and semver boundaries
- **Database Schema**: Relational migration version number

## 2. Analysis & Output Structure

The planner compares `UpgradeGraph` (current) against `UpgradeGraph` (target) and outputs an `UpgradePlan`:

1. **Incompatibilities**:
   - Protocol mismatch: Protocol changes require client-side updates before server-side upgrades.
   - Major component bumps: Flag breaking API changes across framework or server runtime.
   - Package semver breaking upgrades: Breaking bumps (`^0.1` -> `0.2`, `1.0` -> `2.0`).

2. **Client Update Requirements**:
   - Identifies whether client update is optional or mandatory.
   - Protocol bumps and major client bumps strictly mandate client updates prior to connection.

3. **Database Migration Requirements**:
   - Steps required: `|target_version - current_version|`.
   - Forward migration: Safe to apply with backup checkpoint.
   - Destructive / Backward migration: Flags potential data loss.

4. **Rollback Boundaries**:
   - `Safe`: Non-destructive update with complete rollback capability.
   - `Conditional`: Requires schema rollback script or package restore.
   - `Impossible`: Destructive schema changes or incompatible wire changes prevent automated rollback.

5. **Restart Severity Classification**:
   - `None`: No server downtime or restart required.
   - `HotReload`: Safe dynamic reload of sandboxed resource packages without dropping clients.
   - `GracefulServerRestart`: Scheduled server restart allowing client disconnect and state flushing.
   - `FullStackRestart`: Database migration and binary replacement requiring complete cluster reboot.

## 3. Tooling Integration

The `ald` CLI and Aegis web dashboard invoke `UpgradePlanner::plan()` prior to committing any package, framework, or server runtime upgrade.
