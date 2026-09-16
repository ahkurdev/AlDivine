# Backup & Disaster Recovery (DR) Specification

This document defines the backup lifecycle, disaster recovery protocol, database migration safety gates, and `aldivine.lock` integrity enforcement for Project Aldivine.

## 1. Backup Model

Aldivine backups are content-addressed and checksum-verified snapshots containing:
- Server configuration files (`server.cfg`, `server.toml`)
- Resource files and manifests (`ald_manifest.toml`, `fxmanifest.lua`)
- Resource runtime states and local storage
- Lockfile (`aldivine.lock`) pinning all package versions and hashes
- Database migration version stamp (`db_migration_version`)

### Backup Kinds

1. **Full Backup**: Complete standalone archive of all server state, configuration, and migration stamps.
2. **Incremental Backup**: Differential snapshot referencing a specific `parent_backup_id`.

## 2. Integrity Verification

Every file in the backup is SHA-256 hashed. The root `BackupManifest` contains:
- `backup_id`: Unique identifier
- `created_at_utc`: ISO 8601 creation timestamp
- `kind`: Full or Incremental
- `server_version`: Target Aldivine runtime release
- `db_migration_version`: Target relational schema version
- `entries`: Deterministically sorted file paths, sizes, and SHA-256 hashes
- `total_bytes`: Total payload size

Integrity is validated before any extraction or restore operation (`BackupArchive::verify_integrity`). If any single byte differs or an entry is missing, the restore is aborted immediately (`BackupError::HashMismatch`).

## 3. Database Migration Safety Gates

Restoring game files while the database schema has evolved to a newer or incompatible version causes game-state corruption and silent runtime query errors.

### Safe Restore Protocol

1. Read the `db_migration_version` from the backup manifest.
2. Query the live database schema migration version.
3. If `backup_version == target_version`: Restore proceed normally.
4. If `backup_version != target_version`: Restore is blocked by default (`BackupError::IncompatibleDbMigration`).
5. Overriding with `--allow-migration-gap` requires explicit operator confirmation, flags `requires_migration_sync = true`, and prompts the operator to run database rollback/migration tooling prior to booting `ald-server`.

## 4. `aldivine.lock` Enforcement

The lockfile pins exact package dependencies, versions, publisher IDs, and content hashes.
- Tampered packages (modified code or altered hash) fail lock verification (`BackupError::LockHashMismatch`).
- Restoring from backup restores the exact pinned state without unexpected dependency drift.

## 5. RPO & RTO Objectives

- **RPO (Recovery Point Objective)**: Maximum 1 hour between automated differential snapshots.
- **RTO (Recovery Time Objective)**: Sub-5-minute restoration via staged atomic directory replacement.
