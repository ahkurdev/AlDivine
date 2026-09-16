//! Astryn Client — native multiplayer runtime bootstrap.
//!
//! Owns the client lifecycle: config, identity session, AstraNet connection,
//! and the shutdown path. Script runtimes, resource management, and the UI
//! bridge are separate modules wired here as they land.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::watch;

pub mod join;

/// Client runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// Not yet started.
    Idle,
    /// Resolving platform identities / device id.
    Bootstrapping,
    /// Negotiating AstraNet session.
    Connecting,
    /// Session established; resources loading.
    Loading,
    /// Fully in-session.
    Running,
    /// Shutting down.
    Stopping,
    /// Unrecoverable failure. See `last_error`.
    Failed,
}

/// Identity + connection configuration for one Astryn instance.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Target server `host:port`.
    pub server_addr: String,
    /// Aldivine account token presented to the server (never a password).
    pub session_token: Option<String>,
    /// Skip TLS/cert pinning when talking to localhost dev servers.
    pub allow_insecure: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig { server_addr: "127.0.0.1:30120".into(), session_token: None, allow_insecure: true }
    }
}

/// Astryn client handle.
pub struct AstrynClient {
    state: Arc<std::sync::Mutex<ClientState>>,
    config: ClientConfig,
    started_at: Instant,
    shutdown_tx: watch::Sender<bool>,
    /// Background connect task. Awaited on shutdown so the client only
    /// reports Idle once the task has actually stopped.
    task: tokio::task::JoinHandle<()>,
    session: Arc<tokio::sync::Mutex<Option<ConnectedTransport>>>,
    last_event: Arc<std::sync::Mutex<Option<ald_protocol::EventEnvelope>>>,
}

/// Live transport after admission: the same socket/session the handshake used.
pub struct ConnectedTransport {
    pub transport: ald_network::UdpTransport,
    pub peer: std::net::SocketAddr,
    pub session_id: u64,
}

impl AstrynClient {
    /// Bootstrap the client. Does not block: returns once the background
    /// connect task is queued.
    pub fn start(config: ClientConfig) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = Arc::new(std::sync::Mutex::new(ClientState::Bootstrapping));
        let started_at = Instant::now();
        let session = Arc::new(tokio::sync::Mutex::new(None));
        let last_event = Arc::new(std::sync::Mutex::new(None));

        let task_state = Arc::clone(&state);
        let task_session = Arc::clone(&session);
        let task_event = Arc::clone(&last_event);
        let addr = config.server_addr.clone();
        let task = tokio::spawn(async move {
            run_connect_task(task_state, task_session, task_event, addr, shutdown_rx).await;
        });

        AstrynClient { state, config, started_at, shutdown_tx, task, session, last_event }
    }

    pub fn state(&self) -> ClientState {
        *self.state.lock().unwrap()
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Last server event received after admission, if any.
    pub fn last_event(&self) -> Option<ald_protocol::EventEnvelope> {
        self.last_event.lock().unwrap().clone()
    }

    /// True once the handshake completed and the socket is retained.
    pub async fn has_session(&self) -> bool {
        self.session.lock().await.is_some()
    }

    /// Fetch one resource file over the ResourceTransfer channel, reassemble
    /// every chunk, verify its SHA-256 against the negotiated manifest, and
    /// commit it to the content cache. Fails closed on stall, hash mismatch,
    /// or a dead session — never an infinite download.
    pub async fn fetch_resource_file(
        &self,
        name: &str,
        expected: &ald_protocol::ResourceEntry,
        cache_dir: &std::path::Path,
    ) -> anyhow::Result<Vec<u8>> {
        use ald_download::{ChunkPlan, Download};
        use ald_protocol::{Channel, Packet, PacketHeader, TransferRequest, CHUNK_CAP, PROTOCOL_VERSION};

        let mut guard = self.session.lock().await;
        let live = guard.as_mut().ok_or_else(|| anyhow::anyhow!("no session"))?;
        let mut dl = Download::new(ChunkPlan::new(expected.size, CHUNK_CAP).map_err(|e| anyhow::anyhow!("{e}"))?, 0);
        let mut tick = 0u64;
        let mut attempts = 0u64;
        while !dl.is_complete() {
            if attempts > 3 * dl.plan().chunk_count.max(1) + 10 {
                anyhow::bail!("download stalled: {name}");
            }
            let (offset, len) = dl.missing_ranges().into_iter().next().ok_or_else(|| anyhow::anyhow!("no range"))?;
            let req = TransferRequest { name: name.into(), offset, len };
            let payload = ald_protocol::encode_request(&req).map_err(|e| anyhow::anyhow!("{e}"))?;
            let out = Packet {
                header: PacketHeader {
                    protocol_version: PROTOCOL_VERSION,
                    session_id: live.session_id,
                    channel: Channel::ResourceTransfer,
                    sequence: 0,
                    tick: 0,
                    timestamp_ms: 0,
                    flags: 0,
                    payload_len: payload.len() as u32,
                },
                payload,
            };
            live.transport.send(&out, live.peer).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            attempts += 1;
            tick += 1;
            match tokio::time::timeout(std::time::Duration::from_millis(2000), live.transport.recv()).await {
                Ok(Ok((packet, _))) if packet.header.channel == Channel::ResourceTransfer => {
                    let (header, data) =
                        ald_protocol::decode_response(&packet.payload).map_err(|e| anyhow::anyhow!("{e}"))?;
                    if header.name != name || header.total != expected.size {
                        anyhow::bail!("chunk for wrong file");
                    }
                    dl.receive(header.offset, &data, tick).map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                _ => continue,
            }
        }
        let bytes = dl.assembled().ok_or_else(|| anyhow::anyhow!("incomplete"))?.to_vec();
        let digest = ald_cache::sha256_hex(&bytes);
        if digest != expected.hash {
            anyhow::bail!("hash mismatch for {name}");
        }
        let mut cache =
            ald_cache::ContentCache::open(cache_dir, 256 * 1024 * 1024).map_err(|e| anyhow::anyhow!("{e}"))?;
        let staged = cache.stage(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
        cache.commit(staged, &expected.hash).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(bytes)
    }

    /// Request graceful shutdown and wait for the background task to exit.
    /// Takes `&mut self` because the connect task handle can only be awaited
    /// once.
    pub async fn shutdown(&mut self) -> Result<()> {
        {
            let mut s = self.state.lock().unwrap();
            if *s != ClientState::Failed {
                *s = ClientState::Stopping;
            }
        }
        let _ = self.shutdown_tx.send(true);
        // Await the connect task, but never indefinitely: a wedged task must
        // not hang the launcher's teardown path.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut self.task).await;
        *self.state.lock().unwrap() = ClientState::Idle;
        Ok(())
    }
}

/// Background connect task: resolves identities, opens AstraNet, transitions
/// state. Connection failures set `Failed` rather than panicking — the user
/// gets a retry in the launcher instead of a crash.
///
/// The client walks the real admission pipeline and reaches `Running` only
/// after the server sends `ServerReady { world_join: true }`. A bound socket
/// alone never promotes the client past `Connecting`/`Loading`.
async fn run_connect_task(
    state: Arc<std::sync::Mutex<ClientState>>,
    session_slot: Arc<tokio::sync::Mutex<Option<ConnectedTransport>>>,
    event_slot: Arc<std::sync::Mutex<Option<ald_protocol::EventEnvelope>>>,
    addr: String,
    mut shutdown: watch::Receiver<bool>,
) {
    use ald_protocol::{Channel, HandshakeMessage, Packet, PacketHeader, PROTOCOL_VERSION};
    use std::net::SocketAddr;
    use std::time::{SystemTime, UNIX_EPOCH};

    let transition = |s: &Arc<std::sync::Mutex<ClientState>>, next: ClientState| {
        *s.lock().unwrap() = next;
    };
    let now_ms = || SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);

    transition(&state, ClientState::Connecting);

    let transport = match ald_network::UdpTransport::bind("0.0.0.0:0").await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "astryn: failed to bind local socket");
            transition(&state, ClientState::Failed);
            return;
        }
    };

    let peer: SocketAddr = match addr.parse() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "astryn: bad server address");
            transition(&state, ClientState::Failed);
            return;
        }
    };

    let send = |msg: &HandshakeMessage, session_id: u64| {
        let payload = msg.encode().unwrap_or_default();
        Packet {
            header: PacketHeader {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                channel: Channel::Auth,
                sequence: 0,
                tick: 0,
                timestamp_ms: now_ms(),
                flags: 0,
                payload_len: payload.len() as u32,
            },
            payload,
        }
    };

    async fn recv_handshake(
        transport: &ald_network::UdpTransport,
        shutdown: &mut watch::Receiver<bool>,
        timeout_ms: u64,
    ) -> Option<HandshakeMessage> {
        tokio::select! {
            res = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), transport.recv()) => {
                match res {
                    Ok(Ok((packet, _))) => HandshakeMessage::decode(&packet.payload).ok(),
                    _ => None,
                }
            }
            _ = shutdown.changed() => None,
        }
    }

    if *shutdown.borrow() {
        transition(&state, ClientState::Idle);
        return;
    }

    let hello = HandshakeMessage::ClientHello {
        protocol_version: PROTOCOL_VERSION,
        build: 3095,
        edition: "Enhanced".into(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let mut session_id = 0u64;
    let mut nonce = String::new();
    let mut got_hello = false;
    for _ in 0..3 {
        if *shutdown.borrow() {
            transition(&state, ClientState::Idle);
            return;
        }
        let p = send(&hello, 0);
        if transport.send(&p, peer).await.is_err() {
            break;
        }
        // The server answers with ServerHello then AuthChallenge as two packets.
        if let Some(msg) = recv_handshake(&transport, &mut shutdown, 1500).await {
            match msg {
                HandshakeMessage::ServerHello { session_id: sid, .. } => {
                    session_id = sid;
                    if let Some(HandshakeMessage::AuthChallenge { nonce: n }) =
                        recv_handshake(&transport, &mut shutdown, 1500).await
                    {
                        nonce = n;
                        got_hello = true;
                        break;
                    }
                }
                HandshakeMessage::Reject { code, reason } => {
                    tracing::warn!(%code, %reason, "astryn: hello rejected");
                    transition(&state, ClientState::Failed);
                    return;
                }
                _ => {}
            }
        }
    }
    if !got_hello {
        tracing::warn!("astryn: no ServerHello; staying out of Running");
        transition(&state, ClientState::Failed);
        return;
    }

    let aldivine_id = ald_core::AldivinePlayerId::new().to_string();
    let auth = HandshakeMessage::ClientAuth {
        aldivine_id: aldivine_id.clone(),
        token: None,
        platform: None,
        challenge_response: Some(nonce),
    };
    {
        let p = send(&auth, session_id);
        if transport.send(&p, peer).await.is_err() {
            transition(&state, ClientState::Failed);
            return;
        }
    }
    let mut authed = false;
    for _ in 0..4 {
        match recv_handshake(&transport, &mut shutdown, 1500).await {
            Some(HandshakeMessage::AuthResult { accepted: true, .. }) => {
                authed = true;
            }
            Some(HandshakeMessage::IdentityRequest) if authed => break,
            Some(HandshakeMessage::Reject { code, reason }) => {
                tracing::warn!(%code, %reason, "astryn: auth rejected");
                transition(&state, ClientState::Failed);
                return;
            }
            Some(_) => {}
            None => {
                if *shutdown.borrow() {
                    transition(&state, ClientState::Idle);
                    return;
                }
            }
        }
    }
    if !authed {
        transition(&state, ClientState::Failed);
        return;
    }
    transition(&state, ClientState::Loading);

    let id_resp = HandshakeMessage::IdentityResponse {
        aldivine_id: aldivine_id.clone(),
        platform_id: None,
        entitlement: "FREE".into(),
        device_id: Some(format!("ephemeral-{}-{}", std::process::id(), now_ms())),
    };
    {
        let p = send(&id_resp, session_id);
        if transport.send(&p, peer).await.is_err() {
            transition(&state, ClientState::Failed);
            return;
        }
    }
    let mut deferral_done = false;
    for _ in 0..6 {
        match recv_handshake(&transport, &mut shutdown, 1500).await {
            Some(HandshakeMessage::DeferralDone) => {
                deferral_done = true;
                break;
            }
            Some(HandshakeMessage::DeferralUpdate { .. }) => {}
            Some(HandshakeMessage::EntitlementRequest) => {}
            Some(HandshakeMessage::Reject { code, reason }) => {
                tracing::warn!(%code, %reason, "astryn: identity rejected");
                transition(&state, ClientState::Failed);
                return;
            }
            Some(_) => {}
            None => {
                if *shutdown.borrow() {
                    transition(&state, ClientState::Idle);
                    return;
                }
            }
        }
    }
    if !deferral_done {
        transition(&state, ClientState::Failed);
        return;
    }

    {
        let p = send(&HandshakeMessage::EntitlementRequest, session_id);
        let _ = transport.send(&p, peer).await;
    }
    let mut entitled = false;
    for _ in 0..4 {
        match recv_handshake(&transport, &mut shutdown, 1500).await {
            Some(HandshakeMessage::EntitlementResult { .. }) => {
                entitled = true;
                break;
            }
            Some(HandshakeMessage::Reject { code, reason }) => {
                tracing::warn!(%code, %reason, "astryn: entitlement rejected");
                transition(&state, ClientState::Failed);
                return;
            }
            Some(_) => {}
            None => {
                if *shutdown.borrow() {
                    transition(&state, ClientState::Idle);
                    return;
                }
            }
        }
    }
    if !entitled {
        transition(&state, ClientState::Failed);
        return;
    }

    {
        let p = send(&HandshakeMessage::ClientReady, session_id);
        let _ = transport.send(&p, peer).await;
    }
    loop {
        if *shutdown.borrow() {
            transition(&state, ClientState::Idle);
            return;
        }
        match recv_handshake(&transport, &mut shutdown, 2000).await {
            Some(HandshakeMessage::ServerReady { world_join: true }) => {
                // The server follows ServerReady with a welcome event on the
                // EventReliable channel. Capture one packet so the join proves
                // real server-to-client event delivery, then keep the socket.
                if let Ok(Ok((packet, _))) =
                    tokio::time::timeout(std::time::Duration::from_millis(1000), transport.recv()).await
                {
                    if packet.header.channel == Channel::EventReliable {
                        if let Ok(ev) = ald_protocol::decode_event(&packet.payload) {
                            *event_slot.lock().unwrap() = Some(ev);
                        }
                    }
                }
                *session_slot.lock().await = Some(ConnectedTransport { transport, peer, session_id });
                transition(&state, ClientState::Running);
                let _ = shutdown.changed().await;
                transition(&state, ClientState::Idle);
                return;
            }
            Some(HandshakeMessage::ServerReady { world_join: false }) => {}
            Some(HandshakeMessage::QueueUpdate { .. }) => {}
            Some(HandshakeMessage::QueueAdmitted) => {}
            Some(HandshakeMessage::ManifestOffer { .. }) => {}
            Some(HandshakeMessage::Reject { code, reason }) => {
                tracing::warn!(%code, %reason, "astryn: join rejected");
                transition(&state, ClientState::Failed);
                return;
            }
            Some(_) => {}
            None => {
                if *shutdown.borrow() {
                    transition(&state, ClientState::Idle);
                    return;
                }
                tracing::warn!("astryn: join timed out waiting for ServerReady");
                transition(&state, ClientState::Failed);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn no_server_never_reaches_running() {
        let client = AstrynClient::start(ClientConfig { server_addr: "127.0.0.1:30199".into(), ..Default::default() });
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert_ne!(client.state(), ClientState::Running);
        let mut client = client;
        client.shutdown().await.unwrap();
        assert_eq!(client.state(), ClientState::Idle);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bad_address_fails_not_panics() {
        let mut client =
            AstrynClient::start(ClientConfig { server_addr: "not-an-address".into(), ..Default::default() });
        for _ in 0..50 {
            if client.state() == ClientState::Failed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(client.state(), ClientState::Failed);
        // Shutdown still completes cleanly from a failed state.
        client.shutdown().await.unwrap();
    }
}
