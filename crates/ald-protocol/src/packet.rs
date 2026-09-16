use crate::channel::Channel;
use ald_core::AldError;

/// Maximum allowed payload size (16 MiB). Packets exceeding this are rejected
/// before any gameplay code runs.
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;
pub const HEADER_LEN: usize = 32;

pub const FLAG_RELIABLE: u8 = 1;
pub const FLAG_ORDERED: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketHeader {
    pub protocol_version: u16,
    pub session_id: u64,
    pub channel: Channel,
    pub sequence: u32,
    pub tick: u32,
    pub timestamp_ms: u64,
    pub flags: u8,
    pub payload_len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub header: PacketHeader,
    pub payload: Vec<u8>,
}

fn u32_to_array(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Encode header (32 bytes) followed by payload. Validates length.
pub fn encode_packet(p: &Packet) -> Result<Vec<u8>, AldError> {
    if p.payload.len() as u32 != p.header.payload_len {
        return Err(AldError::Protocol("payload_len mismatch".into()));
    }
    if p.header.payload_len > MAX_PAYLOAD {
        return Err(AldError::Protocol("payload exceeds MAX_PAYLOAD".into()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + p.payload.len());
    out.extend_from_slice(&p.header.protocol_version.to_le_bytes());
    out.extend_from_slice(&p.header.session_id.to_le_bytes());
    out.extend_from_slice(&[p.header.channel as u8]);
    out.extend_from_slice(&u32_to_array(p.header.sequence));
    out.extend_from_slice(&u32_to_array(p.header.tick));
    out.extend_from_slice(&p.header.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&[p.header.flags]);
    out.extend_from_slice(&u32_to_array(p.header.payload_len));
    out.extend_from_slice(&p.payload);
    Ok(out)
}

/// Decode a full packet. Expects exactly HEADER_LEN + payload_len bytes.
/// `expected_version` is checked and a version mismatch is rejected.
pub fn decode_packet(bytes: &[u8], expected_version: u16) -> Result<Packet, AldError> {
    if bytes.len() < HEADER_LEN {
        return Err(AldError::Protocol("packet shorter than header".into()));
    }
    let protocol_version = u16::from_le_bytes([bytes[0], bytes[1]]);
    if protocol_version != expected_version {
        return Err(AldError::Protocol(format!(
            "protocol version mismatch: got {protocol_version}, want {expected_version}"
        )));
    }
    let session_id = u64::from_le_bytes(bytes[2..10].try_into().unwrap());
    let channel = Channel::from_u8(bytes[10]).ok_or_else(|| AldError::Protocol("invalid channel".into()))?;
    let sequence = u32::from_le_bytes(bytes[11..15].try_into().unwrap());
    let tick = u32::from_le_bytes(bytes[15..19].try_into().unwrap());
    let timestamp_ms = u64::from_le_bytes(bytes[19..27].try_into().unwrap());
    let flags = bytes[27];
    let payload_len = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
    if payload_len > MAX_PAYLOAD {
        return Err(AldError::Protocol("declared payload exceeds MAX_PAYLOAD".into()));
    }
    let total = HEADER_LEN + payload_len as usize;
    if bytes.len() != total {
        return Err(AldError::Protocol(format!(
            "packet length {} != header {} + payload {}",
            bytes.len(),
            HEADER_LEN,
            payload_len
        )));
    }
    Ok(Packet {
        header: PacketHeader {
            protocol_version,
            session_id,
            channel,
            sequence,
            tick,
            timestamp_ms,
            flags,
            payload_len,
        },
        payload: bytes[HEADER_LEN..total].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;
    use crate::PROTOCOL_VERSION;

    fn sample() -> Packet {
        Packet {
            header: PacketHeader {
                protocol_version: PROTOCOL_VERSION,
                session_id: 0xDEAD_BEEF,
                channel: Channel::EventReliable,
                sequence: 42,
                tick: 7,
                timestamp_ms: 123456,
                flags: FLAG_RELIABLE | FLAG_ORDERED,
                payload_len: 3,
            },
            payload: vec![9, 8, 7],
        }
    }

    #[test]
    fn packet_roundtrip() {
        let p = sample();
        let enc = encode_packet(&p).unwrap();
        assert_eq!(enc.len(), HEADER_LEN + 3);
        let dec = decode_packet(&enc, PROTOCOL_VERSION).unwrap();
        assert_eq!(p, dec);
    }

    #[test]
    fn bad_version_rejected() {
        let enc = encode_packet(&sample()).unwrap();
        assert!(decode_packet(&enc, PROTOCOL_VERSION + 1).is_err());
    }

    #[test]
    fn truncated_payload_rejected() {
        let mut enc = encode_packet(&sample()).unwrap();
        enc.truncate(enc.len() - 1);
        assert!(decode_packet(&enc, PROTOCOL_VERSION).is_err());
    }

    #[test]
    fn oversized_payload_rejected() {
        let mut p = sample();
        p.header.payload_len = MAX_PAYLOAD + 1;
        p.payload = vec![0u8; (MAX_PAYLOAD + 1) as usize];
        assert!(encode_packet(&p).is_err());
    }
}
