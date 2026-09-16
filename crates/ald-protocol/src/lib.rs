//! Aldivine Protocol: wire framing for AstraNet.
//! Implements the packet header, logical channels, flags, and a validated
//! encode/decode path. Payload is opaque bytes; application events are framed
//! via serde_json with length prefix.

pub mod channel;
pub mod event;
pub mod handshake;
pub mod packet;
pub mod transfer;

pub use channel::Channel;
pub use event::{decode_event, encode_event, EventEnvelope};
pub use handshake::{
    HandshakeMessage, HandshakeState, ResourceEntry, REJECT_AUTH, REJECT_DEFERRAL, REJECT_ENTITLEMENT, REJECT_IDENTITY,
    REJECT_PROTOCOL, REJECT_SERVER_FULL,
};
pub use packet::{
    decode_packet, encode_packet, Packet, PacketHeader, FLAG_ORDERED, FLAG_RELIABLE, HEADER_LEN, MAX_PAYLOAD,
};
pub use transfer::{
    decode_request, decode_response, encode_request, encode_response, TransferHeader, TransferRequest, CHUNK_CAP,
    MAX_REQUEST,
};

/// Current wire protocol version. Bump on incompatible changes.
pub const PROTOCOL_VERSION: u16 = 1;
