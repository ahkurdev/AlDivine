//! Versioned world snapshots: encode, verify, decode.
//!
//! A snapshot is the full persistent state at one tick (entity fields,
//! ownership, server-defined world values — the caller flattens its state
//! into entries; this crate frames them). Framing:
//!
//! ```text
//! magic "ALDSNAP1" (8 bytes) | schema u32 BE | tick u64 BE
//! entry count u32 BE, then per entry:
//!   key len u16 BE | key | value len u32 BE | value
//! sha256 over everything before it (32 bytes)
//! ```
//!
//! Decode rejects: wrong magic, unsupported schema, truncation, trailing
//! garbage, checksum mismatch, and oversized entries — each with its own
//! error, so Aegis can tell "no snapshot" from "corrupt snapshot" from
//! "snapshot from a newer Aldivine".

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

/// Snapshot wire magic.
pub const MAGIC: &[u8; 8] = b"ALDSNAP1";
/// Schema versions this build decodes.
pub const SUPPORTED_SCHEMAS: &[u32] = &[1];
/// Longest accepted key / value.
pub const MAX_KEY_LEN: usize = 256;
pub const MAX_VALUE_LEN: usize = 4_096_576; // 4 MiB headroom over callers' 1 MiB

/// The persistent state at one tick, as flat entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub schema_version: u32,
    pub tick: u64,
    pub entries: BTreeMap<String, Vec<u8>>,
}

impl Snapshot {
    pub fn new(tick: u64, entries: BTreeMap<String, Vec<u8>>) -> Self {
        Snapshot { schema_version: 1, tick, entries }
    }

    /// Encode with checksum trailer.
    pub fn encode(&self) -> Result<Vec<u8>, SnapshotError> {
        if !SUPPORTED_SCHEMAS.contains(&self.schema_version) {
            return Err(SnapshotError::UnsupportedSchema(self.schema_version));
        }
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.schema_version.to_be_bytes());
        out.extend_from_slice(&self.tick.to_be_bytes());
        if self.entries.len() > u32::MAX as usize {
            return Err(SnapshotError::TooManyEntries);
        }
        out.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        for (k, v) in &self.entries {
            if k.len() > MAX_KEY_LEN {
                return Err(SnapshotError::KeyTooLong(k.len()));
            }
            if v.len() > MAX_VALUE_LEN {
                return Err(SnapshotError::ValueTooLong(v.len()));
            }
            out.extend_from_slice(&(k.len() as u16).to_be_bytes());
            out.extend_from_slice(k.as_bytes());
            out.extend_from_slice(&(v.len() as u32).to_be_bytes());
            out.extend_from_slice(v);
        }
        let sum = Sha256::digest(&out);
        out.extend_from_slice(&sum);
        Ok(out)
    }

    /// Decode and verify. Fails closed on any structural problem.
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let mut r = Reader::new(bytes);
        let magic = r.take(8).ok_or(SnapshotError::Truncated)?;
        if magic != MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let schema = r.u32().ok_or(SnapshotError::Truncated)?;
        if !SUPPORTED_SCHEMAS.contains(&schema) {
            return Err(SnapshotError::UnsupportedSchema(schema));
        }
        let tick = r.u64().ok_or(SnapshotError::Truncated)?;
        let count = r.u32().ok_or(SnapshotError::Truncated)? as usize;
        if count > 1_000_000 {
            return Err(SnapshotError::TooManyEntries);
        }
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let klen = r.u16().ok_or(SnapshotError::Truncated)? as usize;
            if klen > MAX_KEY_LEN || klen == 0 {
                return Err(SnapshotError::KeyTooLong(klen));
            }
            let kbytes = r.take(klen).ok_or(SnapshotError::Truncated)?;
            let key = String::from_utf8(kbytes.to_vec()).map_err(|_| SnapshotError::BadKeyEncoding)?;
            let vlen = r.u32().ok_or(SnapshotError::Truncated)? as usize;
            if vlen > MAX_VALUE_LEN {
                return Err(SnapshotError::ValueTooLong(vlen));
            }
            let value = r.take(vlen).ok_or(SnapshotError::Truncated)?.to_vec();
            if entries.insert(key, value).is_some() {
                return Err(SnapshotError::DuplicateKey);
            }
        }
        let (body, trailer) = bytes.split_at(bytes.len().saturating_sub(32));
        if trailer.len() != 32 || r.remaining() != 32 {
            return Err(SnapshotError::Truncated);
        }
        let sum = Sha256::digest(body);
        if sum.as_slice() != trailer {
            return Err(SnapshotError::ChecksumMismatch);
        }
        Ok(Snapshot { schema_version: schema, tick, entries })
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SnapshotError {
    #[error("bad snapshot magic")]
    BadMagic,
    #[error("unsupported snapshot schema {0}")]
    UnsupportedSchema(u32),
    #[error("snapshot truncated")]
    Truncated,
    #[error("snapshot checksum mismatch")]
    ChecksumMismatch,
    #[error("key too long ({0} bytes)")]
    KeyTooLong(usize),
    #[error("key is not valid UTF-8")]
    BadKeyEncoding,
    #[error("duplicate key")]
    DuplicateKey,
    #[error("value too long ({0} bytes)")]
    ValueTooLong(usize),
    #[error("too many entries")]
    TooManyEntries,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot::new(
            1234,
            BTreeMap::from([
                ("entity:1:pos".to_string(), vec![1, 2, 3]),
                ("global:weather".to_string(), b"sunny".to_vec()),
                ("empty".to_string(), vec![]),
            ]),
        )
    }

    #[test]
    fn round_trip() {
        let s = sample();
        let bytes = s.encode().unwrap();
        assert_eq!(Snapshot::decode(&bytes).unwrap(), s);
    }

    #[test]
    fn empty_snapshot_round_trips() {
        let s = Snapshot::new(0, BTreeMap::new());
        assert_eq!(Snapshot::decode(&s.encode().unwrap()).unwrap(), s);
    }

    #[test]
    fn truncation_rejected_at_every_cut() {
        let bytes = sample().encode().unwrap();
        // Cut at several structural points, not just the tail.
        for cut in [0, 4, 8, 12, 20, 24, bytes.len() / 2, bytes.len() - 33, bytes.len() - 1] {
            assert!(Snapshot::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn single_flipped_byte_rejected() {
        let mut bytes = sample().encode().unwrap();
        // Flip the last body byte (inside a value): framing still parses,
        // so the checksum is what must catch it.
        let at = bytes.len() - 33;
        bytes[at] ^= 0xFF;
        assert_eq!(Snapshot::decode(&bytes), Err(SnapshotError::ChecksumMismatch));
        // Flip a header byte instead: still rejected (by framing or checksum).
        let mut bytes = sample().encode().unwrap();
        bytes[20] ^= 0xFF;
        assert!(Snapshot::decode(&bytes).is_err());
    }

    #[test]
    fn wrong_magic_rejected() {
        let mut bytes = sample().encode().unwrap();
        bytes[0] = b'X';
        // Magic is checked before the checksum, and the checksum would fail
        // anyway — either way it must not decode.
        assert!(Snapshot::decode(&bytes).is_err());
    }

    #[test]
    fn trailing_garbage_rejected() {
        let mut bytes = sample().encode().unwrap();
        bytes.extend_from_slice(b"extra");
        assert!(Snapshot::decode(&bytes).is_err());
    }

    #[test]
    fn unsupported_schema_rejected_on_both_sides() {
        let mut s = sample();
        s.schema_version = 99;
        assert_eq!(s.encode(), Err(SnapshotError::UnsupportedSchema(99)));
        // Hand-built v99 frame must not decode either.
        let mut bytes = sample().encode().unwrap();
        bytes[8..12].copy_from_slice(&99u32.to_be_bytes());
        assert!(Snapshot::decode(&bytes).is_err());
    }

    #[test]
    fn oversized_entries_rejected() {
        let mut entries = BTreeMap::new();
        entries.insert("k".repeat(MAX_KEY_LEN + 1), vec![]);
        assert!(matches!(Snapshot::new(0, entries).encode(), Err(SnapshotError::KeyTooLong(_))));
        let mut entries = BTreeMap::new();
        entries.insert("k".into(), vec![0; MAX_VALUE_LEN + 1]);
        assert!(matches!(Snapshot::new(0, entries).encode(), Err(SnapshotError::ValueTooLong(_))));
    }

    #[test]
    fn empty_input_rejected() {
        assert_eq!(Snapshot::decode(&[]), Err(SnapshotError::Truncated));
    }
}
