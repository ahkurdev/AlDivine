//! Chunked resource transfer over the ResourceTransfer channel.
//!
//! Wire model: the request is a JSON body; the response is a JSON header line,
//! one `\n`, then the raw chunk bytes. Raw bytes ride outside JSON so chunks
//! stay compact (no base64 blowup) and hashing covers exactly what flies.

use serde::{Deserialize, Serialize};

use ald_core::AldError;

/// Largest chunk the server will serve in one reply (32 KiB).
pub const CHUNK_CAP: u64 = 32 * 1024;

/// Longest request body accepted (small JSON; chunks are never requested inline).
pub const MAX_REQUEST: usize = 4096;

/// One chunk request. `name` is `resource/relative/path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRequest {
    pub name: String,
    pub offset: u64,
    pub len: u64,
}

/// Header leading every chunk reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferHeader {
    pub name: String,
    pub offset: u64,
    pub total: u64,
    pub sha256: String,
}

pub fn encode_request(req: &TransferRequest) -> Result<Vec<u8>, AldError> {
    if req.len > CHUNK_CAP {
        return Err(AldError::Protocol("chunk request exceeds CHUNK_CAP".into()));
    }
    serde_json::to_vec(req).map_err(|e| AldError::Protocol(e.to_string()))
}

pub fn decode_request(bytes: &[u8]) -> Result<TransferRequest, AldError> {
    if bytes.len() > MAX_REQUEST {
        return Err(AldError::Protocol("transfer request too large".into()));
    }
    let req: TransferRequest = serde_json::from_slice(bytes).map_err(|e| AldError::Protocol(e.to_string()))?;
    if req.len > CHUNK_CAP {
        return Err(AldError::Protocol("chunk request exceeds CHUNK_CAP".into()));
    }
    Ok(req)
}

pub fn encode_response(header: &TransferHeader, data: &[u8]) -> Result<Vec<u8>, AldError> {
    if data.len() as u64 > CHUNK_CAP {
        return Err(AldError::Protocol("chunk exceeds CHUNK_CAP".into()));
    }
    let mut head = serde_json::to_vec(header).map_err(|e| AldError::Protocol(e.to_string()))?;
    head.push(b'\n');
    let mut out = Vec::with_capacity(head.len() + data.len());
    out.extend_from_slice(&head);
    out.extend_from_slice(data);
    Ok(out)
}

pub fn decode_response(bytes: &[u8]) -> Result<(TransferHeader, Vec<u8>), AldError> {
    let at = bytes.iter().position(|b| *b == b'\n').ok_or_else(|| AldError::Protocol("chunk missing header".into()))?;
    let header: TransferHeader = serde_json::from_slice(&bytes[..at]).map_err(|e| AldError::Protocol(e.to_string()))?;
    let data = bytes[at + 1..].to_vec();
    if data.len() as u64 > CHUNK_CAP {
        return Err(AldError::Protocol("chunk exceeds CHUNK_CAP".into()));
    }
    Ok((header, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> TransferRequest {
        TransferRequest { name: "spawn/ald_manifest.toml".into(), offset: 0, len: 1024 }
    }

    #[test]
    fn request_roundtrip() {
        let bytes = encode_request(&req()).unwrap();
        assert_eq!(decode_request(&bytes).unwrap(), req());
    }

    #[test]
    fn oversized_request_rejected() {
        let big = TransferRequest { name: "x".into(), offset: 0, len: CHUNK_CAP + 1 };
        assert!(encode_request(&big).is_err());
        let mut raw = serde_json::to_vec(&big).unwrap();
        assert!(decode_request(&raw).is_err());
        raw.clear();
        raw.resize(MAX_REQUEST + 1, b'x');
        assert!(decode_request(&raw).is_err());
    }

    #[test]
    fn response_roundtrip_with_binary() {
        let header = TransferHeader { name: "a/b".into(), offset: 7, total: 100, sha256: "abc".into() };
        let data = vec![0u8, 255, b'\n', 13];
        let bytes = encode_response(&header, &data).unwrap();
        let (h, d) = decode_response(&bytes).unwrap();
        assert_eq!(h, header);
        assert_eq!(d, data);
    }

    #[test]
    fn response_without_newline_rejected() {
        assert!(decode_response(b"{\"no\": \"newline\"}").is_err());
    }

    #[test]
    fn garbage_rejected() {
        assert!(decode_request(b"not json").is_err());
        assert!(decode_response(b"\n").is_err());
    }
}
