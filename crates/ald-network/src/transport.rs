//! AstraNet UDP transport.
//! Hybrid design: UDP datagrams carry framed ald-protocol packets; QUIC
//! streams/datagrams remain an optional transport for reliable bulk transfer
//! (resource downloads). This module implements the UDP path.

use std::net::SocketAddr;

use ald_core::AldError;
use ald_protocol::{decode_packet, encode_packet, Packet, PROTOCOL_VERSION};
use tokio::net::UdpSocket;

/// Max UDP datagram we will read (typical safe MTU minus headers).
pub const MTU: usize = 1400;

/// A bound UDP endpoint that sends/receives framed packets.
pub struct UdpTransport {
    sock: UdpSocket,
}

impl UdpTransport {
    pub async fn bind(addr: &str) -> Result<Self, AldError> {
        let sock = UdpSocket::bind(addr).await.map_err(|e| AldError::Network(format!("bind {addr}: {e}")))?;
        Ok(UdpTransport { sock })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, AldError> {
        self.sock.local_addr().map_err(|e| AldError::Network(format!("local_addr: {e}")))
    }

    /// Send a framed packet to `dst`.
    pub async fn send(&self, p: &Packet, dst: SocketAddr) -> Result<(), AldError> {
        let buf = encode_packet(p)?;
        self.sock.send_to(&buf, dst).await.map_err(|e| AldError::Network(format!("send_to: {e}")))?;
        Ok(())
    }

    /// Receive one datagram, decoding and version-checking it.
    pub async fn recv(&self) -> Result<(Packet, SocketAddr), AldError> {
        let mut buf = vec![0u8; MTU];
        let (n, src) = self.sock.recv_from(&mut buf).await.map_err(|e| AldError::Network(format!("recv_from: {e}")))?;
        let p = decode_packet(&buf[..n], PROTOCOL_VERSION)?;
        Ok((p, src))
    }
}

/// Fragment a payload that exceeds the MTU into ordered fragments.
/// Each fragment is its own framed packet sharing the parent sequence space;
/// reassembly is driven by the caller using the returned indices.
pub fn fragment(payload: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    if payload.is_empty() {
        return vec![Vec::new()];
    }
    payload.chunks(chunk.max(1)).map(|c| c.to_vec()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_splits_and_joins() {
        let payload = vec![7u8; 5000];
        let parts = fragment(&payload, 1400);
        assert!(parts.len() > 1);
        let joined: Vec<u8> = parts.concat();
        assert_eq!(joined, payload);
    }

    #[test]
    fn fragment_empty() {
        let empty: [u8; 0] = [];
        let parts: Vec<Vec<u8>> = fragment(&empty, 100);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].is_empty());
    }
}
