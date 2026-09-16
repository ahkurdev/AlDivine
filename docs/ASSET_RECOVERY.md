# Asset Cache Corruption & Recovery Specification

This document defines content integrity validation, cache repair, and catastrophic recovery workflows for the Aldivine Streaming Engine.

## 1. Corruption Detection

- All cached assets are content-addressed by SHA-256 digest (`crates/ald-cache`).
- Staged writes use temporary filenames (`.tmp`) and atomic rename. Partial downloads are never valid cache entries.
- Pre-mount integrity verification re-computes the file hash before handoff to game thread.

## 2. Recovery Procedures

1. **Hash Mismatch**:
   - If computed hash != manifest hash: purge corrupted chunk from cache immediately.
   - Re-queue chunk with priority lane in `ald-download`.
2. **CDN Mirror Failover**:
   - If download fails after 3 retries or stalls for > 3.0s: switch to secondary CDN origin mirror.
3. **Cache Quota & Repair**:
   - CLI command `ald cache repair` scans all cached artifacts against disk blocks, purging orphan or truncated blobs.
   - LRU eviction preserves pinned base-resource assets and actively mounted world models.
