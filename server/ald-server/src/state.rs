//! Shared server state: config, resource lifecycle, telemetry, shutdown.

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ald_config::Config;
use ald_resource::LifecycleManager;
use ald_telemetry::MetricStore;
use tokio::sync::watch;

use crate::admission::AdmissionManager;
use crate::console::ConsoleCommand;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Snapshot of live server state. Shared via Arc across tasks.
pub struct ServerState {
    config: Config,
    started_at: Instant,
    /// Resource lifecycle states. Guarded: mutated only through the
    /// lifecycle command channel to keep state transitions single-writer.
    lifecycle: Arc<tokio::sync::Mutex<LifecycleManager>>,
    metrics: Arc<MetricStore>,
    admission: Arc<tokio::sync::Mutex<AdmissionManager>>,
    framework_players: Arc<tokio::sync::Mutex<aldivine_framework::PlayerService>>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    console_tx: tokio::sync::mpsc::UnboundedSender<ConsoleCommand>,
    /// Held here (not dropped) so Aegis/operator tooling can inject commands
    /// via `console_tx()`; the console task takes it once via
    /// `take_console_rx()` and serves it alongside stdin.
    console_rx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<ConsoleCommand>>>,
}

impl ServerState {
    pub fn new(config: Config, shutdown_tx: watch::Sender<bool>) -> Self {
        let shutdown_rx = shutdown_tx.subscribe();
        let (console_tx, console_rx) = tokio::sync::mpsc::unbounded_channel();
        let identity_req = ald_identity::IdentityRequirements {
            require_aldivine_account: config.identity.require_aldivine_account,
            require_gta_entitlement: config.identity.require_gta_entitlement,
            require_rockstar: config.identity.require_rockstar,
            require_steam: config.identity.require_steam,
            require_epic: config.identity.require_epic,
            require_device_id: config.identity.require_device_id,
            record_connection_ip: config.identity.record_connection_ip,
        };
        let admission = AdmissionManager::new(config.server.name.clone(), config.server.max_players, identity_req);
        ServerState {
            config,
            started_at: Instant::now(),
            lifecycle: Arc::new(tokio::sync::Mutex::new(LifecycleManager::new())),
            metrics: Arc::new(MetricStore::new()),
            admission: Arc::new(tokio::sync::Mutex::new(admission)),
            framework_players: Arc::new(tokio::sync::Mutex::new(aldivine_framework::PlayerService::new())),
            shutdown_tx,
            shutdown_rx,
            console_tx,
            console_rx: std::sync::Mutex::new(Some(console_rx)),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn metrics(&self) -> &Arc<MetricStore> {
        &self.metrics
    }
    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }
    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }
    /// Signal shutdown to every task watching the flag. Idempotent best
    /// effort: silently ignored if no task is watching yet.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
    pub fn console_tx(&self) -> tokio::sync::mpsc::UnboundedSender<ConsoleCommand> {
        self.console_tx.clone()
    }
    /// Take the remote-command receiver. `None` if the console task (or a
    /// previous caller) already took it — there is exactly one server.
    pub fn take_console_rx(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<ConsoleCommand>> {
        self.console_rx.lock().ok()?.take()
    }
    pub fn lifecycle(&self) -> &Arc<tokio::sync::Mutex<LifecycleManager>> {
        &self.lifecycle
    }
    pub fn admission(&self) -> &Arc<tokio::sync::Mutex<AdmissionManager>> {
        &self.admission
    }
    pub fn framework_players(&self) -> &Arc<tokio::sync::Mutex<aldivine_framework::PlayerService>> {
        &self.framework_players
    }

    /// Number of resources currently in the Running state.
    pub async fn running_count(&self) -> usize {
        let l = self.lifecycle.lock().await;
        l.running_count()
    }

    /// Periodic telemetry sampling. Bounded: exits once shutdown is signalled.
    pub async fn telemetry_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.tick().await; // discard immediate first tick
        loop {
            interval.tick().await;
            if *self.shutdown_rx.borrow() {
                break;
            }
            self.metrics.gauge("server.uptime_s", self.uptime().as_secs() as f64);
            self.metrics.gauge("resources.running", self.running_count().await as f64);
        }
    }

    /// Network receive loop: decode, session lookup, channel dispatch.
    /// Errors are logged and counted, never fatal — a single bad peer must
    /// not take the server down. Unknown packets are rejected, never panic.
    pub async fn network_loop(self: Arc<Self>, listener: &ald_network::UdpTransport) {
        use ald_protocol::{Channel, HandshakeMessage, Packet, PacketHeader, PROTOCOL_VERSION};
        let mut rx = self.shutdown_rx();
        loop {
            if *rx.borrow() {
                break;
            }
            tokio::select! {
                res = listener.recv() => {
                    match res {
                        Ok((packet, peer)) => {
                            self.metrics.incr("network.packets", 1);
                            self.metrics.incr("network.bytes", packet.payload.len() as u64);
                            match packet.header.channel {
                                Channel::Auth => {
                                    match HandshakeMessage::decode(&packet.payload) {
                                        Ok(msg) => {
                                            let remote = peer.ip().to_string();
                                            let t = now_ms();
                                            let responses = {
                                                let mut adm = self.admission.lock().await;
                                                adm.handle(peer, msg, &remote, t)
                                            };
                                            for r in responses {
                                                if matches!(&r, HandshakeMessage::ServerReady { world_join: true }) {
                                                    let id_opt = {
                                                        let adm = self.admission.lock().await;
                                                        adm.aldivine_id_of(&peer)
                                                    };
                                                    if let Some(id_s) = id_opt {
                                                        if let Ok(pid) = ald_core::AldivinePlayerId::from_string(&id_s) {
                                                            let mut fw = self.framework_players.lock().await;
                                                            fw.join(pid);
                                                            self.metrics.incr("framework.players", 1);
                                                        }
                                                    }
                                                }
                                                match r.encode() {
                                                    Ok(payload) => {
                                                        let sid = {
                                                            let adm = self.admission.lock().await;
                                                            adm.session_id_of(&peer)
                                                        };
                                                        let out = Packet {
                                                            header: PacketHeader {
                                                                protocol_version: PROTOCOL_VERSION,
                                                                session_id: sid,
                                                                channel: Channel::Auth,
                                                                sequence: 0,
                                                                tick: 0,
                                                                timestamp_ms: now_ms(),
                                                                flags: 0,
                                                                payload_len: payload.len() as u32,
                                                            },
                                                            payload,
                                                        };
                                                        if let Err(e) = listener.send(&out, peer).await {
                                                            self.metrics.incr("network.errors", 1);
                                                            tracing::warn!(%peer, error = %e, "auth reply failed");
                                                        }
                                                    }
                                                    Err(e) => {
                                                        self.metrics.incr("network.errors", 1);
                                                        tracing::warn!(%peer, error = %e, "handshake encode failed");
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            self.metrics.incr("network.errors", 1);
                                            tracing::warn!(%peer, error = %e, "bad handshake payload");
                                        }
                                    }
                                }
                                Channel::Heartbeat => {
                                    self.metrics.incr("network.heartbeat", 1);
                                }
                                Channel::Control
                                | Channel::EventReliable
                                | Channel::EventUnreliable
                                | Channel::EntityState
                                | Channel::ResourceTransfer
                                | Channel::Admin
                                | Channel::VoiceMetadata => {
                                    self.metrics.incr("network.channel_pkts", 1);
                                    tracing::debug!(%peer, ch = ?packet.header.channel, "channel packet counted");
                                }
                            }
                        }
                        Err(e) => {
                            self.metrics.incr("network.errors", 1);
                            tracing::warn!(error = %e, "udp recv error");
                        }
                    }
                }
                _ = rx.changed() => break,
            }
        }
    }
}
