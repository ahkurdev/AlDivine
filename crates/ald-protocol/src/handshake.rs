//! AstraNet handshake protocol: the admission state machine shared by Astryn and ald-server.
//!
//! Wire model. The packet payload on the `Auth` channel carries one of these
//! messages serialized with `serde_json`. The packet header carries the
//! protocol version and session id; the payload carries the handshake step.
//!
//! The client drives the steps in order; the server answers each one. The
//! client is admitted to the world only after `ServerReady { world_join: true }`,
//! never from a successful socket bind alone.
//!
//! Alignment with the real admission pipeline:
//! ClientHello -> ServerHello -> AuthChallenge -> ClientAuth -> Identity ->
//! Entitlement -> Deferrals -> Queue -> Manifest -> Ready.

use serde::{Deserialize, Serialize};

/// Handshake error codes safe to send to the client. Namespaced `ALD-`.
pub const REJECT_PROTOCOL: &str = "ALD-PROTO-001";
pub const REJECT_AUTH: &str = "ALD-AUTH-001";
pub const REJECT_IDENTITY: &str = "ALD-IDENTITY-001";
pub const REJECT_ENTITLEMENT: &str = "ALD-ENT-001";
pub const REJECT_SERVER_FULL: &str = "ALD-QUEUE-001";
pub const REJECT_DEFERRAL: &str = "ALD-QUEUE-002";

/// Server-side admission state. One per peer session.
///
/// `Ready` is reachable only by walking every earlier step. There is no
/// shortcut from a bound socket to `Ready`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandshakeState {
    Idle,
    WaitingHello,
    WaitingAuth,
    WaitingIdentity,
    WaitingEntitlement,
    WaitingDeferrals,
    WaitingQueue,
    WaitingManifest,
    Ready,
    Rejected,
    Failed,
}

impl HandshakeState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, HandshakeState::Ready | HandshakeState::Rejected | HandshakeState::Failed)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            HandshakeState::Idle => "IDLE",
            HandshakeState::WaitingHello => "WAITING_HELLO",
            HandshakeState::WaitingAuth => "WAITING_AUTH",
            HandshakeState::WaitingIdentity => "WAITING_IDENTITY",
            HandshakeState::WaitingEntitlement => "WAITING_ENTITLEMENT",
            HandshakeState::WaitingDeferrals => "WAITING_DEFERRALS",
            HandshakeState::WaitingQueue => "WAITING_QUEUE",
            HandshakeState::WaitingManifest => "WAITING_MANIFEST",
            HandshakeState::Ready => "READY",
            HandshakeState::Rejected => "REJECTED",
            HandshakeState::Failed => "FAILED",
        }
    }
}

/// One entry in the negotiated resource manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceEntry {
    pub name: String,
    /// hex SHA-256 of the resource package bytes.
    pub hash: String,
    pub size: u64,
}

/// Messages on the wire. Each variant tags under `t`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum HandshakeMessage {
    ClientHello {
        protocol_version: u16,
        build: u32,
        edition: String,
        client_version: String,
    },
    ServerHello {
        protocol_version: u16,
        session_id: u64,
        server_name: String,
        max_players: u32,
        features: Vec<String>,
    },
    /// Challenge the client must answer: nonce the server generated.
    AuthChallenge {
        nonce: String,
    },
    /// Client's answer. `aldivine_id` is the account id, `token` an opaque
    /// session token when the account server issued one.
    ClientAuth {
        aldivine_id: String,
        token: Option<String>,
        platform: Option<String>,
        /// Answer to the server's nonce challenge.
        challenge_response: Option<String>,
    },
    AuthResult {
        accepted: bool,
        message: String,
    },
    IdentityRequest,
    IdentityResponse {
        aldivine_id: String,
        platform_id: Option<String>,
        entitlement: String,
        /// Client-generated device identifier. Ephemeral ids are accepted
        /// with low confidence; absence fails servers that require device id.
        #[serde(default)]
        device_id: Option<String>,
    },
    EntitlementRequest,
    EntitlementResult {
        state: String,
        message: String,
    },
    DeferralUpdate {
        message: String,
    },
    DeferralDone,
    QueueUpdate {
        position: usize,
        total_queued: usize,
    },
    QueueAdmitted,
    ManifestOffer {
        resources: Vec<ResourceEntry>,
    },
    ClientReady,
    /// Final admission. `world_join: true` is the only path into the world.
    ServerReady {
        world_join: bool,
    },
    Reject {
        code: String,
        reason: String,
    },
}

impl HandshakeMessage {
    /// Serialize for a packet payload.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let msg: HandshakeMessage = serde_json::from_slice(bytes)?;
        Ok(msg)
    }

    /// Serde internal tagging does not support `#[serde(other)]`.
    /// Unknown tags are rejected at decode time: fail-closed, never silently dropped.
    pub fn tag(&self) -> &'static str {
        match self {
            HandshakeMessage::ClientHello { .. } => "ClientHello",
            HandshakeMessage::ServerHello { .. } => "ServerHello",
            HandshakeMessage::AuthChallenge { .. } => "AuthChallenge",
            HandshakeMessage::ClientAuth { .. } => "ClientAuth",
            HandshakeMessage::AuthResult { .. } => "AuthResult",
            HandshakeMessage::IdentityRequest => "IdentityRequest",
            HandshakeMessage::IdentityResponse { .. } => "IdentityResponse",
            HandshakeMessage::EntitlementRequest => "EntitlementRequest",
            HandshakeMessage::EntitlementResult { .. } => "EntitlementResult",
            HandshakeMessage::DeferralUpdate { .. } => "DeferralUpdate",
            HandshakeMessage::DeferralDone => "DeferralDone",
            HandshakeMessage::QueueUpdate { .. } => "QueueUpdate",
            HandshakeMessage::QueueAdmitted => "QueueAdmitted",
            HandshakeMessage::ManifestOffer { .. } => "ManifestOffer",
            HandshakeMessage::ClientReady => "ClientReady",
            HandshakeMessage::ServerReady { .. } => "ServerReady",
            HandshakeMessage::Reject { .. } => "Reject",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_client_hello() {
        let m = HandshakeMessage::ClientHello {
            protocol_version: 1,
            build: 3095,
            edition: "Enhanced".into(),
            client_version: "0.1.0".into(),
        };
        let bytes = m.encode().unwrap();
        let back = HandshakeMessage::decode(&bytes).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn roundtrip_server_ready() {
        let m = HandshakeMessage::ServerReady { world_join: true };
        let bytes = m.encode().unwrap();
        let back = HandshakeMessage::decode(&bytes).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn roundtrip_manifest_offer() {
        let m = HandshakeMessage::ManifestOffer {
            resources: vec![ResourceEntry { name: "spawn".into(), hash: "abc".into(), size: 12 }],
        };
        let bytes = m.encode().unwrap();
        assert_eq!(HandshakeMessage::decode(&bytes).unwrap(), m);
    }

    #[test]
    fn unknown_tag_rejected() {
        let json = r#"{"t":"FutureHandshakeStep","d":{"x":1}}"#;
        assert!(HandshakeMessage::decode(json.as_bytes()).is_err());
    }

    #[test]
    fn garbage_payload_errors() {
        assert!(HandshakeMessage::decode(b"not json at all").is_err());
    }

    #[test]
    fn identity_response_without_device_id_still_decodes() {
        let json = r#"{"t":"IdentityResponse","d":{"aldivine_id":"x","platform_id":null,"entitlement":"FREE"}}"#;
        match HandshakeMessage::decode(json.as_bytes()).unwrap() {
            HandshakeMessage::IdentityResponse { device_id: None, .. } => {}
            other => panic!("expected IdentityResponse with device_id None, got {other:?}"),
        }
    }

    #[test]
    fn state_machine_order() {
        use HandshakeState::*;
        // Ready only follows the full walk; no shortcut from Idle.
        assert!(!Idle.is_terminal());
        assert!(!WaitingHello.is_terminal());
        assert!(Ready.is_terminal());
        assert!(Rejected.is_terminal());
        assert!(Failed.is_terminal());
    }

    #[test]
    fn tags_named() {
        assert_eq!(HandshakeMessage::ClientReady.tag(), "ClientReady");
        let r = HandshakeMessage::Reject { code: "x".into(), reason: "y".into() };
        assert_eq!(r.tag(), "Reject");
    }
}
