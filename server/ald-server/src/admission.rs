use std::collections::HashMap;
use std::net::SocketAddr;

use ald_deferrals::{DeferralContext, DeferralSession, GateDecision, SessionOutcome};
use ald_identity::pipeline::check_identity_policy;
use ald_identity::{IdentityRequirements, PlayerIdentity};
use ald_protocol::PROTOCOL_VERSION;
use ald_protocol::{
    HandshakeMessage, HandshakeState, ResourceEntry, REJECT_AUTH, REJECT_DEFERRAL, REJECT_ENTITLEMENT, REJECT_IDENTITY,
    REJECT_PROTOCOL, REJECT_SERVER_FULL,
};
use ald_queue::{JoinOutcome, JoinRequest, Queue, QueueConfig, RejectReason};

pub struct AdmissionSession {
    pub state: HandshakeState,
    pub session_id: u64,
    pub aldivine_id: Option<String>,
    pub platform: Option<String>,
    pub nonce: String,
    pub polls: u64,
    pub queue_position: Option<usize>,
}

impl AdmissionSession {
    fn new(session_id: u64, nonce: String) -> Self {
        AdmissionSession {
            state: HandshakeState::WaitingAuth,
            session_id,
            aldivine_id: None,
            platform: None,
            nonce,
            polls: 0,
            queue_position: None,
        }
    }
}

pub struct AdmissionManager {
    server_name: String,
    max_players: u32,
    next_session: u64,
    sessions: HashMap<SocketAddr, AdmissionSession>,
    queue: Queue,
    manifest: Vec<ResourceEntry>,
    identity_req: IdentityRequirements,
}

impl AdmissionManager {
    pub fn new(server_name: String, max_players: u32, identity_req: IdentityRequirements) -> Self {
        let cfg = QueueConfig::new(max_players as usize, 0, 64, 300_000).unwrap_or(QueueConfig {
            max_players: max_players as usize,
            reserved_slots: 0,
            max_queue_len: 64,
            stale_after_ms: 300_000,
        });
        AdmissionManager {
            server_name,
            max_players,
            next_session: 1,
            sessions: HashMap::new(),
            queue: Queue::new(cfg),
            manifest: Vec::new(),
            identity_req,
        }
    }

    pub fn set_manifest(&mut self, entries: Vec<ResourceEntry>) {
        self.manifest = entries;
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn state_of(&self, peer: &SocketAddr) -> Option<HandshakeState> {
        self.sessions.get(peer).map(|s| s.state)
    }

    pub fn session_id_of(&self, peer: &SocketAddr) -> u64 {
        self.sessions.get(peer).map(|s| s.session_id).unwrap_or(0)
    }

    pub fn aldivine_id_of(&self, peer: &SocketAddr) -> Option<String> {
        self.sessions.get(peer).and_then(|s| s.aldivine_id.clone())
    }

    pub fn remove(&mut self, peer: &SocketAddr) -> bool {
        if let Some(s) = self.sessions.remove(peer) {
            if let Some(id) = s.aldivine_id {
                self.queue.playing_leave(&id);
                self.queue.queue_leave(&id);
            }
            true
        } else {
            false
        }
    }

    fn reject(code: &str, reason: &str) -> HandshakeMessage {
        HandshakeMessage::Reject { code: code.into(), reason: reason.into() }
    }

    pub fn handle(
        &mut self,
        peer: SocketAddr,
        msg: HandshakeMessage,
        remote_ip: &str,
        now_ms: u64,
    ) -> Vec<HandshakeMessage> {
        match msg {
            HandshakeMessage::ClientHello { protocol_version, .. } => self.on_hello(peer, protocol_version),
            HandshakeMessage::ClientAuth { aldivine_id, token: _, platform, challenge_response } => {
                self.on_auth(peer, aldivine_id, platform, challenge_response, remote_ip)
            }
            HandshakeMessage::IdentityResponse { aldivine_id, platform_id: _, entitlement: _, device_id } => {
                self.on_identity(peer, aldivine_id, device_id, remote_ip, now_ms)
            }
            HandshakeMessage::EntitlementRequest => self.on_entitlement(peer),
            HandshakeMessage::ClientReady => self.on_ready(peer, now_ms),
            HandshakeMessage::DeferralDone
            | HandshakeMessage::QueueAdmitted
            | HandshakeMessage::AuthResult { .. }
            | HandshakeMessage::ServerHello { .. }
            | HandshakeMessage::AuthChallenge { .. }
            | HandshakeMessage::IdentityRequest
            | HandshakeMessage::EntitlementResult { .. }
            | HandshakeMessage::DeferralUpdate { .. }
            | HandshakeMessage::QueueUpdate { .. }
            | HandshakeMessage::ManifestOffer { .. }
            | HandshakeMessage::ServerReady { .. }
            | HandshakeMessage::Reject { .. } => {
                vec![Self::reject(REJECT_PROTOCOL, "out-of-order message for this session")]
            }
        }
    }

    fn on_hello(&mut self, peer: SocketAddr, version: u16) -> Vec<HandshakeMessage> {
        if version != PROTOCOL_VERSION {
            return vec![Self::reject(REJECT_PROTOCOL, "protocol version mismatch")];
        }
        let id = self.next_session;
        self.next_session += 1;
        let nonce = format!("n-{id}-{peer}");
        self.sessions.insert(peer, AdmissionSession::new(id, nonce.clone()));
        vec![
            HandshakeMessage::ServerHello {
                protocol_version: PROTOCOL_VERSION,
                session_id: id,
                server_name: self.server_name.clone(),
                max_players: self.max_players,
                features: vec!["deferrals".into(), "queue".into(), "manifest".into()],
            },
            HandshakeMessage::AuthChallenge { nonce },
        ]
    }

    fn on_auth(
        &mut self,
        peer: SocketAddr,
        aldivine_id: String,
        platform: Option<String>,
        challenge_response: Option<String>,
        _remote_ip: &str,
    ) -> Vec<HandshakeMessage> {
        let s = match self.sessions.get_mut(&peer) {
            Some(s) => s,
            None => return vec![Self::reject(REJECT_PROTOCOL, "auth before hello")],
        };
        if s.state != HandshakeState::WaitingAuth {
            return vec![Self::reject(REJECT_PROTOCOL, "auth out of order")];
        }
        if aldivine_id.trim().is_empty() {
            s.state = HandshakeState::Rejected;
            return vec![Self::reject(REJECT_AUTH, "missing account id")];
        }
        match challenge_response {
            Some(r) if r == s.nonce => {}
            _ => {
                s.state = HandshakeState::Rejected;
                return vec![Self::reject(REJECT_AUTH, "challenge failed")];
            }
        }
        s.aldivine_id = Some(aldivine_id);
        s.platform = platform;
        s.state = HandshakeState::WaitingIdentity;
        vec![
            HandshakeMessage::AuthResult { accepted: true, message: "auth ok".into() },
            HandshakeMessage::IdentityRequest,
        ]
    }

    fn on_identity(
        &mut self,
        peer: SocketAddr,
        aldivine_id: String,
        device_id: Option<String>,
        remote_ip: &str,
        now_ms: u64,
    ) -> Vec<HandshakeMessage> {
        let sid = match self.sessions.get(&peer) {
            Some(s) => s.session_id,
            None => return vec![Self::reject(REJECT_PROTOCOL, "identity before hello")],
        };
        let parsed =
            ald_core::AldivinePlayerId::from_string(&aldivine_id).unwrap_or_else(|_| ald_core::AldivinePlayerId::new());
        let mut identity = PlayerIdentity::new(parsed);
        identity.network.remote_ip = remote_ip.to_string();
        if let Some(did) = device_id.filter(|d| !d.trim().is_empty()) {
            identity.device = Some(ald_identity::DeviceIdentity {
                device_id: did,
                confidence: "low".to_string(),
                version: 1,
                first_seen: now_ms,
                last_seen: now_ms,
            });
        }
        match check_identity_policy(&identity, &self.identity_req) {
            ald_identity::PolicyOutcome::Pass => {}
            ald_identity::PolicyOutcome::Reject(r) => {
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.state = HandshakeState::Rejected;
                }
                return vec![Self::reject(REJECT_IDENTITY, &r.reason)];
            }
        }
        if let Some(s) = self.sessions.get_mut(&peer) {
            s.state = HandshakeState::WaitingDeferrals;
        }
        let ctx = DeferralContext { player_id: aldivine_id, remote_ip: remote_ip.into(), queue_position: None };
        let mut session =
            DeferralSession::new(vec![Box::new(ald_deferrals::FnGate::new("ban-check", |_| GateDecision::Pass))], 10);
        match session.poll(&ctx) {
            SessionOutcome::Admit => {
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.state = HandshakeState::WaitingQueue;
                }
                let _ = sid;
                vec![HandshakeMessage::DeferralDone, HandshakeMessage::EntitlementRequest]
            }
            SessionOutcome::Waiting { message, .. } => {
                vec![HandshakeMessage::DeferralUpdate { message }]
            }
            SessionOutcome::Denied { code, reason, .. } => {
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.state = HandshakeState::Rejected;
                }
                vec![Self::reject(&code, &reason)]
            }
        }
    }

    fn on_entitlement(&mut self, peer: SocketAddr) -> Vec<HandshakeMessage> {
        let s = match self.sessions.get_mut(&peer) {
            Some(s) => s,
            None => return vec![Self::reject(REJECT_PROTOCOL, "entitlement before hello")],
        };
        if s.state != HandshakeState::WaitingQueue && s.state != HandshakeState::WaitingDeferrals {
            return vec![Self::reject(REJECT_ENTITLEMENT, "entitlement out of order")];
        }
        vec![HandshakeMessage::EntitlementResult { state: "FREE".into(), message: "ok".into() }]
    }

    fn on_ready(&mut self, peer: SocketAddr, now_ms: u64) -> Vec<HandshakeMessage> {
        let (id, state) = match self.sessions.get(&peer) {
            Some(s) => (s.aldivine_id.clone().unwrap_or_default(), s.state),
            None => return vec![Self::reject(REJECT_PROTOCOL, "ready before hello")],
        };
        if id.is_empty() {
            return vec![Self::reject(REJECT_AUTH, "unknown session")];
        }
        if state == HandshakeState::Ready {
            return vec![HandshakeMessage::ServerReady { world_join: true }];
        }
        let outcome = self.queue.join(JoinRequest::new(&id, false), now_ms);
        match outcome {
            JoinOutcome::Admitted => {
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.state = HandshakeState::WaitingManifest;
                }
                let mut out = vec![HandshakeMessage::QueueAdmitted];
                out.push(HandshakeMessage::ManifestOffer { resources: self.manifest.clone() });
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.state = HandshakeState::Ready;
                }
                out.push(HandshakeMessage::ServerReady { world_join: true });
                out
            }
            JoinOutcome::Queued { position } => {
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.queue_position = Some(position);
                }
                vec![
                    HandshakeMessage::QueueUpdate { position, total_queued: self.queue.queue_len() },
                    HandshakeMessage::ManifestOffer { resources: self.manifest.clone() },
                ]
            }
            JoinOutcome::Rejected { reason } => {
                if let Some(s) = self.sessions.get_mut(&peer) {
                    s.state = HandshakeState::Rejected;
                }
                let why = match reason {
                    RejectReason::ServerFull | RejectReason::QueueFull => {
                        (REJECT_SERVER_FULL, "server full, try again later")
                    }
                    RejectReason::AlreadyPresent => (REJECT_DEFERRAL, "already connected"),
                    RejectReason::InvalidId => (REJECT_AUTH, "invalid account id"),
                };
                vec![Self::reject(why.0, why.1)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> AdmissionManager {
        AdmissionManager::new(
            "test".into(),
            64,
            IdentityRequirements {
                require_aldivine_account: false,
                require_gta_entitlement: false,
                require_rockstar: false,
                require_steam: false,
                require_epic: false,
                require_device_id: false,
                record_connection_ip: true,
            },
        )
    }

    fn peer(n: u8) -> SocketAddr {
        format!("127.0.0.1:{}", 40000 + n as u16).parse().unwrap()
    }

    #[test]
    fn hello_then_auth_challenge() {
        let mut m = mgr();
        let p = peer(1);
        let out = m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build: 3095,
                edition: "E".into(),
                client_version: "0.1".into(),
            },
            "127.0.0.1",
            0,
        );
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], HandshakeMessage::ServerHello { .. }));
        assert!(matches!(out[1], HandshakeMessage::AuthChallenge { .. }));
    }

    #[test]
    fn bad_version_rejected() {
        let mut m = mgr();
        let p = peer(2);
        let out = m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: 999,
                build: 0,
                edition: "".into(),
                client_version: "".into(),
            },
            "127.0.0.1",
            0,
        );
        assert!(matches!(out[0], HandshakeMessage::Reject { .. }));
    }

    #[test]
    fn auth_requires_hello_first() {
        let mut m = mgr();
        let p = peer(3);
        let out = m.handle(
            p,
            HandshakeMessage::ClientAuth {
                aldivine_id: "x".into(),
                token: None,
                platform: None,
                challenge_response: Some("n".into()),
            },
            "127.0.0.1",
            0,
        );
        assert!(matches!(out[0], HandshakeMessage::Reject { .. }));
    }

    #[test]
    fn full_walk_to_ready() {
        let mut m = mgr();
        let p = peer(4);
        let id = ald_core::AldivinePlayerId::new().to_string();
        let out = m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build: 3095,
                edition: "E".into(),
                client_version: "0.1".into(),
            },
            "127.0.0.1",
            0,
        );
        let nonce = match &out[1] {
            HandshakeMessage::AuthChallenge { nonce } => nonce.clone(),
            _ => panic!("expected challenge"),
        };
        let out = m.handle(
            p,
            HandshakeMessage::ClientAuth {
                aldivine_id: id.clone(),
                token: None,
                platform: None,
                challenge_response: Some(nonce),
            },
            "127.0.0.1",
            1,
        );
        assert!(matches!(out[0], HandshakeMessage::AuthResult { accepted: true, .. }));
        let out = m.handle(
            p,
            HandshakeMessage::IdentityResponse {
                aldivine_id: id.clone(),
                platform_id: None,
                entitlement: "FREE".into(),
                device_id: None,
            },
            "127.0.0.1",
            2,
        );
        assert!(out.iter().any(|x| matches!(x, HandshakeMessage::DeferralDone)));
        let out = m.handle(p, HandshakeMessage::EntitlementRequest, "127.0.0.1", 3);
        assert!(matches!(out[0], HandshakeMessage::EntitlementResult { .. }));
        let out = m.handle(p, HandshakeMessage::ClientReady, "127.0.0.1", 4);
        assert!(out.iter().any(|x| matches!(x, HandshakeMessage::ServerReady { world_join: true })));
        assert_eq!(m.state_of(&p), Some(HandshakeState::Ready));
    }

    #[test]
    fn wrong_challenge_rejected() {
        let mut m = mgr();
        let p = peer(5);
        let id = ald_core::AldivinePlayerId::new().to_string();
        m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build: 0,
                edition: "".into(),
                client_version: "".into(),
            },
            "127.0.0.1",
            0,
        );
        let out = m.handle(
            p,
            HandshakeMessage::ClientAuth {
                aldivine_id: id,
                token: None,
                platform: None,
                challenge_response: Some("wrong".into()),
            },
            "127.0.0.1",
            1,
        );
        assert!(matches!(out[0], HandshakeMessage::Reject { .. }));
        assert_eq!(m.state_of(&p), Some(HandshakeState::Rejected));
    }

    #[test]
    fn server_full_rejects_when_no_slots() {
        let mut m = AdmissionManager::new(
            "t".into(),
            1,
            IdentityRequirements {
                require_aldivine_account: false,
                require_gta_entitlement: false,
                require_rockstar: false,
                require_steam: false,
                require_epic: false,
                require_device_id: false,
                record_connection_ip: true,
            },
        );
        m.queue = Queue::new(QueueConfig { max_players: 0, reserved_slots: 0, max_queue_len: 0, stale_after_ms: 0 });
        let p = peer(6);
        let id = ald_core::AldivinePlayerId::new().to_string();
        m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build: 0,
                edition: "".into(),
                client_version: "".into(),
            },
            "127.0.0.1",
            0,
        );
        let out1 = m.handle(
            p,
            HandshakeMessage::ClientAuth {
                aldivine_id: id.clone(),
                token: None,
                platform: None,
                challenge_response: Some(format!("n-1-{p}")),
            },
            "127.0.0.1",
            1,
        );
        assert!(matches!(out1[0], HandshakeMessage::AuthResult { .. }));
        m.handle(
            p,
            HandshakeMessage::IdentityResponse {
                aldivine_id: id.clone(),
                platform_id: None,
                entitlement: "FREE".into(),
                device_id: None,
            },
            "127.0.0.1",
            2,
        );
        let out = m.handle(p, HandshakeMessage::ClientReady, "127.0.0.1", 3);
        assert!(out.iter().any(|x| matches!(x, HandshakeMessage::Reject { .. })));
    }

    #[test]
    fn disconnect_frees_session() {
        let mut m = mgr();
        let p = peer(7);
        m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build: 0,
                edition: "".into(),
                client_version: "".into(),
            },
            "127.0.0.1",
            0,
        );
        assert_eq!(m.session_count(), 1);
        assert!(m.remove(&p));
        assert_eq!(m.session_count(), 0);
    }

    fn walk_to_identity(
        m: &mut AdmissionManager,
        p: SocketAddr,
        id: &str,
        device_id: Option<String>,
        now: u64,
    ) -> Vec<HandshakeMessage> {
        let out = m.handle(
            p,
            HandshakeMessage::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build: 3095,
                edition: "E".into(),
                client_version: "0.1".into(),
            },
            "127.0.0.1",
            now,
        );
        let nonce = match &out[1] {
            HandshakeMessage::AuthChallenge { nonce } => nonce.clone(),
            _ => panic!("expected challenge"),
        };
        m.handle(
            p,
            HandshakeMessage::ClientAuth {
                aldivine_id: id.into(),
                token: None,
                platform: None,
                challenge_response: Some(nonce),
            },
            "127.0.0.1",
            now + 1,
        );
        m.handle(
            p,
            HandshakeMessage::IdentityResponse {
                aldivine_id: id.into(),
                platform_id: None,
                entitlement: "FREE".into(),
                device_id,
            },
            "127.0.0.1",
            now + 2,
        )
    }

    #[test]
    fn device_required_rejects_without_device_id() {
        let mut m = AdmissionManager::new(
            "t".into(),
            64,
            IdentityRequirements {
                require_aldivine_account: false,
                require_gta_entitlement: false,
                require_rockstar: false,
                require_steam: false,
                require_epic: false,
                require_device_id: true,
                record_connection_ip: true,
            },
        );
        let p = peer(8);
        let id = ald_core::AldivinePlayerId::new().to_string();
        let out = walk_to_identity(&mut m, p, &id, None, 10);
        assert!(out.iter().any(|x| matches!(x, HandshakeMessage::Reject { .. })));
        assert_eq!(m.state_of(&p), Some(HandshakeState::Rejected));
    }

    #[test]
    fn device_required_passes_with_device_id() {
        let mut m = AdmissionManager::new(
            "t".into(),
            64,
            IdentityRequirements {
                require_aldivine_account: false,
                require_gta_entitlement: false,
                require_rockstar: false,
                require_steam: false,
                require_epic: false,
                require_device_id: true,
                record_connection_ip: true,
            },
        );
        let p = peer(9);
        let id = ald_core::AldivinePlayerId::new().to_string();
        let out = walk_to_identity(&mut m, p, &id, Some("dev-1".into()), 20);
        assert!(out.iter().any(|x| matches!(x, HandshakeMessage::DeferralDone)));
    }
}
