//! Application event framing over AstraNet.
//! Events are length-prefixed JSON envelopes: u32 LE length + serde_json body.

use ald_core::AldError;
use serde::{Deserialize, Serialize};

use crate::MAX_PAYLOAD;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub source_resource: String,
    pub event: String,
    pub data: serde_json::Value,
    pub seq: u64,
}

impl EventEnvelope {
    pub fn new(source_resource: impl Into<String>, event: impl Into<String>, data: serde_json::Value) -> Self {
        Self { source_resource: source_resource.into(), event: event.into(), data, seq: 0 }
    }
}

/// Encode an event as length-prefixed JSON (u32 LE length + json bytes).
pub fn encode_event(e: &EventEnvelope) -> Result<Vec<u8>, AldError> {
    let json = serde_json::to_vec(e).map_err(|e| AldError::Protocol(e.to_string()))?;
    if json.len() > MAX_PAYLOAD as usize {
        return Err(AldError::Protocol("event too large".into()));
    }
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&json);
    Ok(out)
}

/// Decode a length-prefixed JSON event.
pub fn decode_event(bytes: &[u8]) -> Result<EventEnvelope, AldError> {
    if bytes.len() < 4 {
        return Err(AldError::Protocol("event frame too short".into()));
    }
    let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() != len + 4 {
        return Err(AldError::Protocol("event frame length mismatch".into()));
    }
    serde_json::from_slice(&bytes[4..]).map_err(|e| AldError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_roundtrip() {
        let e = EventEnvelope::new("core", "player_joined", json!({"id": 5}));
        let enc = encode_event(&e).unwrap();
        let dec = decode_event(&enc).unwrap();
        assert_eq!(e, dec);
    }

    #[test]
    fn event_truncated_fails() {
        assert!(decode_event(&[1, 0, 0]).is_err());
    }
}
