//! Crash-safe persistence: journal + atomic checkpoints + recovery.
//!
//! Layout inside one directory (`<server>/persistence/`):
//!
//! ```text
//! journal.log     append-only op log (Put/Delete with per-record checksum)
//! checkpoint.bin  last full snapshot, written atomically (tmp file + rename)
//! ```
//!
//! Crash model:
//! - A torn journal tail (partial last write from a crash mid-append) is
//!   detected by record framing + checksum, dropped, and **reported** —
//!   recovery succeeds with `torn_tail_dropped: true`, never silently.
//! - A checksum failure in the *middle* of the journal is corruption, not a
//!   torn tail: recovery fails with [`RecoveryError::Corrupt`]. Only the
//!   final record may be torn.
//! - Checkpoints validate before use; a torn checkpoint is ignored in favor
//!   of journal replay from scratch (`checkpoint_ignored: true`), and a
//!   corrupt (non-torn) checkpoint fails recovery — a bad base must not
//!   silently poison replay.
//! - [`Journal::checkpoint`] truncates the journal after a successful atomic
//!   write, so the log cannot grow forever and stale-owner state is compacted
//!   away. The caller's payload must reflect every op appended so far
//!   (documented contract, enforced by sequence accounting in tests).
//!
//! Sequences start at 1. `next_seq` is max assigned + 1 across checkpoint and
//! journal, so replayed ops never reuse a sequence.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Longest accepted key / value on the write path.
pub const MAX_KEY_LEN: usize = 256;
pub const MAX_VALUE_LEN: usize = 1_048_576; // 1 MiB

const JOURNAL_NAME: &str = "journal.log";
const CHECKPOINT_NAME: &str = "checkpoint.bin";
const CHECKPOINT_TMP: &str = "checkpoint.bin.tmp";
const CHECKPOINT_MAGIC: &[u8; 8] = b"ALDCKPT1";

/// A journaled mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub op: Op,
    pub key: String,
    pub value: Vec<u8>,
}

/// Operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Put,
    Delete,
}

/// What recovery found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// Checkpoint base, if a valid one existed.
    pub checkpoint: Option<(u64, Vec<u8>)>,
    /// Journal records after the checkpoint, in order.
    pub replayed: Vec<Record>,
    /// A torn final journal record was dropped (crash mid-append).
    pub torn_tail_dropped: bool,
    /// A torn checkpoint file was ignored (replayed from scratch instead).
    pub checkpoint_ignored: bool,
    /// Next sequence to assign (max seen + 1, minimum 1).
    pub next_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PersistError {
    #[error("io: {0}")]
    Io(String),
    #[error("bad key ({0})")]
    BadKey(String),
    #[error("value too large ({0} bytes)")]
    TooLarge(usize),
    #[error("journal corrupt at byte offset {0}")]
    Corrupt(u64),
    #[error("checkpoint corrupt")]
    BadCheckpoint,
}

impl From<std::io::Error> for PersistError {
    fn from(e: std::io::Error) -> Self {
        PersistError::Io(e.to_string())
    }
}

/// Append-only journal in `dir`. Owns sequence assignment.
pub struct Journal {
    dir: PathBuf,
    next_seq: u64,
}

impl Journal {
    /// Open (creating `dir`), scanning existing state for `next_seq` without
    /// replaying: opening never fails on a torn tail — that is recovery's job.
    pub fn open(dir: &Path) -> Result<Self, PersistError> {
        std::fs::create_dir_all(dir)?;
        let mut next_seq = 1;
        if let Some((seq, _)) = read_checkpoint(dir)? {
            next_seq = next_seq.max(seq + 1);
        }
        let (records, _, _) = read_journal(dir)?;
        for r in &records {
            next_seq = next_seq.max(r.seq + 1);
        }
        Ok(Journal { dir: dir.to_path_buf(), next_seq })
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    fn validate(key: &str, value: &[u8]) -> Result<(), PersistError> {
        if key.is_empty() || key.len() > MAX_KEY_LEN || key.contains('\0') {
            return Err(PersistError::BadKey(format!("len {}", key.len())));
        }
        if value.len() > MAX_VALUE_LEN {
            return Err(PersistError::TooLarge(value.len()));
        }
        Ok(())
    }

    pub fn append(&mut self, op: Op, key: &str, value: &[u8]) -> Result<u64, PersistError> {
        Self::validate(key, value)?;
        let seq = self.next_seq;
        self.next_seq += 1;
        let mut f = OpenOptions::new().create(true).append(true).open(self.dir.join(JOURNAL_NAME))?;
        f.write_all(&encode_record(seq, op, key, value))?;
        f.sync_all()?;
        Ok(seq)
    }

    /// Atomically store `payload` as the new base at the current tip
    /// (last assigned sequence, or 0 when empty), then truncate the journal.
    /// Returns the checkpoint sequence. The payload must reflect every op
    /// appended so far — recovery replays only what comes after.
    pub fn checkpoint(&mut self, payload: &[u8]) -> Result<u64, PersistError> {
        let seq = self.next_seq - 1;
        let tmp = self.dir.join(CHECKPOINT_TMP);
        let mut f = File::create(&tmp)?;
        let mut body = Vec::new();
        body.extend_from_slice(CHECKPOINT_MAGIC);
        body.extend_from_slice(&seq.to_be_bytes());
        body.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        body.extend_from_slice(payload);
        let sum = Sha256::digest(&body);
        f.write_all(&body)?;
        f.write_all(&sum)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, self.dir.join(CHECKPOINT_NAME))?;
        // Truncate: base now covers everything through `seq`.
        File::create(self.dir.join(JOURNAL_NAME))?.sync_all()?;
        Ok(seq)
    }

    /// Full recovery: validated checkpoint + replay after its sequence.
    pub fn recover(dir: &Path) -> Result<Recovery, PersistError> {
        let (checkpoint, checkpoint_ignored) = match read_checkpoint(dir)? {
            Some(v) => (Some(v), false),
            None => (None, checkpoint_file_present(dir)),
        };
        let base_seq = checkpoint.as_ref().map(|(s, _)| *s).unwrap_or(0);
        let (records, torn_tail_dropped, _) = read_journal(dir)?;
        // Records at or below the base are pre-checkpoint leftovers only if
        // truncation raced a crash; replay strictly after the base.
        let mut replayed: Vec<Record> = records.into_iter().filter(|r| r.seq > base_seq).collect();
        replayed.sort_by_key(|r| r.seq);
        let max = replayed.last().map(|r| r.seq).unwrap_or(base_seq);
        Ok(Recovery { checkpoint, replayed, torn_tail_dropped, checkpoint_ignored, next_seq: max + 1 })
    }
}

fn checkpoint_file_present(dir: &Path) -> bool {
    // A present-but-unreadable checkpoint means it was torn (read_checkpoint
    // returns Ok(None) for torn files). Absent file = fresh server.
    matches!(std::fs::metadata(dir.join(CHECKPOINT_NAME)), Ok(m) if m.is_file())
}

/// Read and validate the checkpoint. `Ok(None)` = absent OR torn tail
/// (truncated file). Non-torn garbage = `Err(BadCheckpoint)`.
fn read_checkpoint(dir: &Path) -> Result<Option<(u64, Vec<u8>)>, PersistError> {
    let path = dir.join(CHECKPOINT_NAME);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // Minimum: magic + seq + len + checksum.
    if bytes.len() < 8 + 8 + 8 + 32 {
        return Ok(None); // torn: too short to even frame
    }
    if &bytes[..8] != CHECKPOINT_MAGIC {
        return Err(PersistError::BadCheckpoint);
    }
    let seq = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let len = u64::from_be_bytes(bytes[16..24].try_into().unwrap()) as usize;
    if len > 64 * 1024 * 1024 {
        return Err(PersistError::BadCheckpoint);
    }
    let total = 24 + len + 32;
    if bytes.len() < total {
        return Ok(None); // torn mid-payload
    }
    if bytes.len() > total {
        return Err(PersistError::BadCheckpoint); // trailing garbage: not a tear
    }
    let (body, sum) = bytes.split_at(total - 32);
    if Sha256::digest(body).as_slice() != sum {
        // Full-length but bad checksum: if the payload region itself is
        // short... it isn't (length matched) — this is corruption.
        // Distinguish a torn write (file shorter than framed) — already
        // handled above — so here: corrupt.
        return Err(PersistError::BadCheckpoint);
    }
    Ok(Some((seq, bytes[24..24 + len].to_vec())))
}

/// Read the journal. Returns (complete records, torn_tail_dropped, end offset).
/// A bad checksum mid-file is corruption; at the exact end with a short
/// remainder it is still corruption unless the record is structurally
/// incomplete (torn). Rule: incomplete framing = torn tail (drop, report);
/// complete framing with bad checksum = corrupt (fail).
fn read_journal(dir: &Path) -> Result<(Vec<Record>, bool, u64), PersistError> {
    let path = dir.join(JOURNAL_NAME);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], false, 0)),
        Err(e) => return Err(e.into()),
    };
    let mut records = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        match decode_record(&bytes[pos..]) {
            Ok((rec, used)) => {
                records.push(rec);
                pos += used;
            }
            Err(RecordDecode::Torn) => {
                // Only acceptable at the tail — and we ARE at the tail of
                // what remains, by construction of the loop.
                return Ok((records, true, pos as u64));
            }
            Err(RecordDecode::Corrupt) => return Err(PersistError::Corrupt(pos as u64)),
        }
    }
    Ok((records, false, pos as u64))
}

enum RecordDecode {
    Torn,
    Corrupt,
}

fn encode_record(seq: u64, op: Op, key: &str, value: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&seq.to_be_bytes());
    body.push(match op {
        Op::Put => 1,
        Op::Delete => 2,
    });
    body.extend_from_slice(&(key.len() as u16).to_be_bytes());
    body.extend_from_slice(key.as_bytes());
    body.extend_from_slice(&(value.len() as u32).to_be_bytes());
    body.extend_from_slice(value);
    let sum = Sha256::digest(&body);
    body.extend_from_slice(&sum);
    // Length-prefix the checksummed body so framing never depends on walking.
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

fn decode_record(buf: &[u8]) -> Result<(Record, usize), RecordDecode> {
    use RecordDecode::{Corrupt, Torn};
    if buf.len() < 4 {
        return Err(Torn);
    }
    let total = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
    if total > 4 + 8 + 1 + 2 + MAX_KEY_LEN + 4 + MAX_VALUE_LEN + 32 {
        return Err(Corrupt);
    }
    if buf.len() < 4 + total {
        return Err(Torn);
    }
    let body = &buf[4..4 + total];
    let (fields, sum) = body.split_at(total - 32);
    if Sha256::digest(fields).as_slice() != sum {
        // Framing complete but checksum bad. Could this be a torn tail that
        // happens to be framing-complete? A crash mid-append leaves a SHORT
        // file, not a full-length record with wrong bytes — treat as corrupt.
        // Exception: zeroed tail from a torn page write still fails checksum
        // and is indistinguishable; failing closed is correct either way.
        return Err(Corrupt);
    }
    let seq = u64::from_be_bytes(fields[0..8].try_into().map_err(|_| Corrupt)?);
    if seq == 0 {
        return Err(Corrupt);
    }
    let op = match fields[8] {
        1 => Op::Put,
        2 => Op::Delete,
        _ => return Err(Corrupt),
    };
    let klen = u16::from_be_bytes(fields[9..11].try_into().map_err(|_| Corrupt)?) as usize;
    if klen == 0 || klen > MAX_KEY_LEN {
        return Err(Corrupt);
    }
    let kend = 11 + klen;
    let key = std::str::from_utf8(fields.get(11..kend).ok_or(Corrupt)?).map_err(|_| Corrupt)?.to_string();
    let vlen = u32::from_be_bytes(fields.get(kend..kend + 4).ok_or(Corrupt)?.try_into().map_err(|_| Corrupt)?) as usize;
    if vlen > MAX_VALUE_LEN {
        return Err(Corrupt);
    }
    let vstart = kend + 4;
    let value = fields.get(vstart..vstart + vlen).ok_or(Corrupt)?.to_vec();
    if vstart + vlen != fields.len() {
        return Err(Corrupt);
    }
    Ok((Record { seq, op, key, value }, 4 + total))
}

/// Apply a recovery to a plain map. Helper for tests and simple consumers;
///
/// production state builders apply `replayed` to their own structures.
pub fn apply_to_map(recovery: &Recovery, base: &mut BTreeMap<String, Vec<u8>>) {
    if let Some((_, payload)) = &recovery.checkpoint {
        // Payload is opaque here; callers decode their own snapshot format.
        let _ = payload;
    }
    for r in &recovery.replayed {
        match r.op {
            Op::Put => {
                base.insert(r.key.clone(), r.value.clone());
            }
            Op::Delete => {
                base.remove(&r.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_IDS: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let id = TEST_IDS.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ald-persist-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn append_recover_roundtrip() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        assert_eq!(j.next_seq(), 1);
        j.append(Op::Put, "a", b"1").unwrap();
        j.append(Op::Put, "b", b"2").unwrap();
        j.append(Op::Delete, "a", b"").unwrap();
        let rec = Journal::recover(&dir).unwrap();
        assert_eq!(rec.checkpoint, None);
        assert!(!rec.torn_tail_dropped && !rec.checkpoint_ignored);
        assert_eq!(rec.next_seq, 4);
        let mut map = BTreeMap::new();
        apply_to_map(&rec, &mut map);
        assert_eq!(map.get("a"), None);
        assert_eq!(map.get("b"), Some(&b"2".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn torn_tail_dropped_and_reported() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        j.append(Op::Put, "a", b"1").unwrap();
        j.append(Op::Put, "b", b"2").unwrap();
        // Simulate crash mid-append: half a record.
        {
            let mut f = OpenOptions::new().append(true).open(dir.join(JOURNAL_NAME)).unwrap();
            let partial = encode_record(999, Op::Put, "c", b"3");
            f.write_all(&partial[..partial.len() / 2]).unwrap();
        }
        let rec = Journal::recover(&dir).unwrap();
        assert!(rec.torn_tail_dropped);
        assert_eq!(rec.replayed.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mid_file_corruption_fails() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        j.append(Op::Put, "a", b"1").unwrap();
        j.append(Op::Put, "b", b"2").unwrap();
        // Flip a byte inside the FIRST record (framing stays complete).
        let path = dir.join(JOURNAL_NAME);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[10] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(Journal::recover(&dir), Err(PersistError::Corrupt(0))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_truncates_and_recovery_replays_after() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        j.append(Op::Put, "a", b"1").unwrap();
        j.append(Op::Put, "b", b"2").unwrap();
        let cp_seq = j.checkpoint(b"snapshot-bytes").unwrap();
        assert_eq!(cp_seq, 2);
        // Journal truncated; new ops continue the sequence.
        let s3 = j.append(Op::Put, "c", b"3").unwrap();
        assert_eq!(s3, 3);
        let rec = Journal::recover(&dir).unwrap();
        assert_eq!(rec.checkpoint, Some((2, b"snapshot-bytes".to_vec())));
        assert_eq!(rec.replayed.len(), 1);
        assert_eq!(rec.replayed[0].key, "c");
        assert_eq!(rec.next_seq, 4);
        // Reopening continues sequences without reuse.
        let mut j2 = Journal::open(&dir).unwrap();
        assert_eq!(j2.next_seq(), 4);
        assert_eq!(j2.append(Op::Put, "d", b"4").unwrap(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn torn_checkpoint_ignored_journal_replays() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        j.append(Op::Put, "a", b"1").unwrap();
        j.checkpoint(b"base").unwrap();
        // Crash mid-checkpoint: truncate the file.
        let path = dir.join(CHECKPOINT_NAME);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..10]).unwrap();
        let rec = Journal::recover(&dir).unwrap();
        assert_eq!(rec.checkpoint, None);
        assert!(rec.checkpoint_ignored);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_checkpoint_fails_closed() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        j.append(Op::Put, "a", b"1").unwrap();
        j.checkpoint(b"base").unwrap();
        // Full-length garbage with valid magic: corrupt, not torn.
        let path = dir.join(CHECKPOINT_NAME);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(Journal::recover(&dir), Err(PersistError::BadCheckpoint));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_path_validated() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        assert!(matches!(j.append(Op::Put, "", b"x"), Err(PersistError::BadKey(_))));
        assert!(matches!(j.append(Op::Put, "a\0b", b"x"), Err(PersistError::BadKey(_))));
        assert!(matches!(j.append(Op::Put, "k", &vec![0; MAX_VALUE_LEN + 1]), Err(PersistError::TooLarge(_))));
        assert_eq!(j.next_seq(), 1); // failed appends consume no sequence
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_dir_recovers_empty() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let rec = Journal::recover(&dir).unwrap();
        assert_eq!(rec.checkpoint, None);
        assert!(rec.replayed.is_empty());
        assert_eq!(rec.next_seq, 1);
        assert!(!rec.torn_tail_dropped && !rec.checkpoint_ignored);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_state_compacted_by_checkpoint() {
        let dir = test_dir();
        let mut j = Journal::open(&dir).unwrap();
        // Churn then delete: journal holds dead history until checkpoint.
        for i in 0..20u64 {
            j.append(Op::Put, &format!("temp{i}"), b"x").unwrap();
        }
        for i in 0..20u64 {
            j.append(Op::Delete, &format!("temp{i}"), b"").unwrap();
        }
        let before = std::fs::metadata(dir.join(JOURNAL_NAME)).unwrap().len();
        assert!(before > 0);
        j.checkpoint(b"compact-base").unwrap();
        let after = std::fs::metadata(dir.join(JOURNAL_NAME)).unwrap().len();
        assert_eq!(after, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
