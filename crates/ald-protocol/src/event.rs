//! Application event framing over AstraNet.
//!
//! Event arguments carry Citizen-compatible msgpack (`pack_cfx`/`unpack_cfx`),
//! which preserves vector/quat ext types and nil-vs-false. The envelope itself
//! stays JSON so it remains inspectable by tooling; only the `data` payload is
//! msgpack-encoded as an opaque byte string.
//!
//! Events are length-prefixed: u32 LE length + body.

use ald_core::AldError;
use ald_serialization_compat::{pack as pack_cfx, unpack as unpack_cfx, CfxValue};
use serde::{Deserialize, Serialize};

use crate::MAX_PAYLOAD;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub source_resource: String,
    pub event: String,
    /// msgpack-encoded CfxValue payload (kept as bytes; serde moves it through).
    #[serde(with = "cfx_bytes")]
    pub data: Vec<u8>,
    pub seq: u64,
}

mod cfx_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        Vec::<u8>::deserialize(d)
    }
}

impl EventEnvelope {
    /// Build an event whose payload is any CfxValue, msgpack-encoded.
    pub fn with_cfx(
        source_resource: impl Into<String>,
        event: impl Into<String>,
        data: &CfxValue,
    ) -> Result<Self, AldError> {
        Ok(Self { source_resource: source_resource.into(), event: event.into(), data: pack_cfx(data)?, seq: 0 })
    }

    /// Decode the event payload as a CfxValue.
    pub fn cfx_data(&self) -> Result<CfxValue, AldError> {
        unpack_cfx(&self.data)
    }

    /// Legacy JSON construction, for paths that deliberately want JSON semantics
    /// (NUI bridges, tooling). Not the cross-runtime event wire.
    pub fn with_json(
        source_resource: impl Into<String>,
        event: impl Into<String>,
        data: serde_json::Value,
    ) -> Result<Self, AldError> {
        Ok(Self {
            source_resource: source_resource.into(),
            event: event.into(),
            data: serde_json::to_vec(&data).map_err(|e| AldError::Protocol(e.to_string()))?,
            seq: 0,
        })
    }

    pub fn json_data(&self) -> Result<serde_json::Value, AldError> {
        serde_json::from_slice(&self.data).map_err(|e| AldError::Protocol(e.to_string()))
    }
}

/// Encode an event as length-prefixed bytes (u32 LE length + body).
pub fn encode_event(e: &EventEnvelope) -> Result<Vec<u8>, AldError> {
    let body = serde_json::to_vec(e).map_err(|e| AldError::Protocol(e.to_string()))?;
    if body.len() > MAX_PAYLOAD as usize {
        return Err(AldError::Protocol("event too large".into()));
    }
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a length-prefixed event.
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
    use ald_serialization_compat::CfxValue;
    use serde_json::json;

    #[test]
    fn event_roundtrip_cfx() {
        let payload = CfxValue::Map(vec![
            (CfxValue::Str("pos".into()), CfxValue::Vector3(1.0, 2.0, 3.0)),
            (CfxValue::Str("hp".into()), CfxValue::Int(100)),
        ]);
        let e = EventEnvelope::with_cfx("core", "player_moved", &payload).unwrap();
        let enc = encode_event(&e).unwrap();
        let dec = decode_event(&enc).unwrap();
        assert_eq!(e, dec);
        // Vectors survive end-to-end, unlike a JSON pipe.
        assert_eq!(dec.cfx_data().unwrap(), payload);
    }

    #[test]
    fn event_roundtrip_json_legacy() {
        let e = EventEnvelope::with_json("core", "player_joined", json!({"id": 5})).unwrap();
        let enc = encode_event(&e).unwrap();
        let dec = decode_event(&enc).unwrap();
        assert_eq!(dec.json_data().unwrap(), json!({"id": 5}));
    }

    #[test]
    fn vector_survives_wire_not_flattened() {
        let v = CfxValue::Vector3(10.5, 0.0, -4.25);
        let e = EventEnvelope::with_cfx("veh", "spawn", &v).unwrap();
        let dec = decode_event(&encode_event(&e).unwrap()).unwrap();
        assert!(dec.cfx_data().unwrap().is_vector());
    }

    #[test]
    fn event_truncated_fails() {
        assert!(decode_event(&[1, 0, 0]).is_err());
    }
}
