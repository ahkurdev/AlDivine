//! AstraSim — Astryn Headless Simulation & Load-Test Client
//!
//! Provides deterministic headless client simulation for multiplayer stress testing:
//! - Handshake & identity negotiation
//! - Queue admission & backpressure
//! - Resource manifest synchronization
//! - Deterministic entity movement & pathing
//! - Reliable/unreliable event transmission
//! - Interest management & dimension transitions
//! - Deterministic packet loss & latency injection
//! - Network disconnects & reconnect resumption

use serde::{Deserialize, Serialize};

/// Deterministic 64-bit XorShift pseudo-random generator.
#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        let s = if seed == 0 { 0x853c49e6748fea9b } else { seed };
        Self { state: s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    pub fn next_bounded(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() as f64 / u64::MAX as f64) as f32
    }
}

/// Lifecycle state of a simulated headless client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimState {
    Disconnected,
    Handshake { attempt: u32 },
    InQueue { position: u32 },
    ResourceNegotiation { total: u32, loaded: u32 },
    InWorld { x: f32, y: f32, z: f32, heading: f32, dimension: u32 },
    Reconnecting { retry_tick: u64 },
}

/// A simulated event sent or received by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimEvent {
    pub name: String,
    pub payload_bytes: usize,
    pub reliable: bool,
}

/// State and telemetry for a single headless simulated client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatedClient {
    pub client_id: u32,
    pub guid: String,
    pub state: SimState,
    pub ticks_active: u64,
    pub events_sent: u64,
    pub events_received: u64,
    pub packets_dropped: u64,
    pub reconnects_completed: u32,
}

impl SimulatedClient {
    pub fn new(client_id: u32) -> Self {
        Self {
            client_id,
            guid: format!("astrasim-guid-{:08x}", client_id),
            state: SimState::Disconnected,
            ticks_active: 0,
            events_sent: 0,
            events_received: 0,
            packets_dropped: 0,
            reconnects_completed: 0,
        }
    }

    pub fn is_in_world(&self) -> bool {
        matches!(self.state, SimState::InWorld { .. })
    }
}

/// Scenario configuration for the simulation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioConfig {
    pub client_count: usize,
    pub seed: u64,
    /// Packet loss in parts-per-thousand (0..1000). 50 = 5% loss.
    pub packet_loss_permille: u32,
    /// Disconnect probability in parts-per-thousand per tick (e.g. 5 = 0.5% per tick).
    pub disconnect_permille: u32,
    pub queue_depth: u32,
    pub manifest_resource_count: u32,
    pub dimension_count: u32,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            client_count: 10,
            seed: 1337,
            packet_loss_permille: 0,
            disconnect_permille: 0,
            queue_depth: 0,
            manifest_resource_count: 5,
            dimension_count: 1,
        }
    }
}

/// Aggregated metrics across all clients in the simulation cluster.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterMetrics {
    pub current_tick: u64,
    pub total_events_sent: u64,
    pub total_events_received: u64,
    pub total_packets_dropped: u64,
    pub clients_in_world: usize,
    pub clients_in_queue: usize,
    pub clients_reconnecting: usize,
}

/// Deterministic cluster managing N simulated clients.
pub struct SimulationCluster {
    pub config: ScenarioConfig,
    rng: DeterministicRng,
    clients: Vec<SimulatedClient>,
    current_tick: u64,
    queue_waitlist: Vec<u32>,
}

impl SimulationCluster {
    pub fn new(config: ScenarioConfig) -> Self {
        let rng = DeterministicRng::new(config.seed);
        let mut clients = Vec::with_capacity(config.client_count);
        for id in 0..config.client_count as u32 {
            clients.push(SimulatedClient::new(id));
        }

        Self { config, rng, clients, current_tick: 0, queue_waitlist: Vec::new() }
    }

    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    pub fn clients(&self) -> &[SimulatedClient] {
        &self.clients
    }

    /// Advance the simulation by one virtual tick.
    pub fn step(&mut self) {
        self.current_tick += 1;
        let tick = self.current_tick;

        // 1. Process queue promotion
        if !self.queue_waitlist.is_empty() && tick.is_multiple_of(2) {
            let promoted_id = self.queue_waitlist.remove(0);
            if let Some(c) = self.clients.iter_mut().find(|c| c.client_id == promoted_id) {
                if let SimState::InQueue { .. } = c.state {
                    c.state = SimState::ResourceNegotiation { total: self.config.manifest_resource_count, loaded: 0 };
                }
            }
            // Update remaining queue positions
            for (idx, &qid) in self.queue_waitlist.iter().enumerate() {
                if let Some(c) = self.clients.iter_mut().find(|c| c.client_id == qid) {
                    c.state = SimState::InQueue { position: (idx + 1) as u32 };
                }
            }
        }

        // 2. Step each client
        for client in &mut self.clients {
            client.ticks_active += 1;

            match client.state {
                SimState::Disconnected => {
                    // Initiate handshake
                    client.state = SimState::Handshake { attempt: 1 };
                }
                SimState::Handshake { attempt } => {
                    if self.config.queue_depth > 0 && self.queue_waitlist.len() < self.config.queue_depth as usize {
                        self.queue_waitlist.push(client.client_id);
                        let pos = self.queue_waitlist.len() as u32;
                        client.state = SimState::InQueue { position: pos };
                    } else if attempt >= 2 {
                        // Handshake completed, start resource download
                        client.state =
                            SimState::ResourceNegotiation { total: self.config.manifest_resource_count, loaded: 0 };
                    } else {
                        client.state = SimState::Handshake { attempt: attempt + 1 };
                    }
                }
                SimState::InQueue { .. } => {
                    // Waiting for queue step above
                }
                SimState::ResourceNegotiation { total, loaded } => {
                    if loaded + 1 >= total {
                        // Spawn in world at seed-derived coords
                        let initial_x = (client.client_id as f32 * 10.0) % 500.0;
                        let initial_y = (client.client_id as f32 * 15.0) % 500.0;
                        let dim = client.client_id % self.config.dimension_count.max(1);
                        client.state =
                            SimState::InWorld { x: initial_x, y: initial_y, z: 72.0, heading: 0.0, dimension: dim };
                    } else {
                        client.state = SimState::ResourceNegotiation { total, loaded: loaded + 1 };
                    }
                }
                SimState::InWorld { ref mut x, ref mut y, ref mut heading, ref mut dimension, .. } => {
                    // Check for disconnect injection
                    let roll_dc = (self.rng.next_bounded(1000)) as u32;
                    if roll_dc < self.config.disconnect_permille {
                        client.state = SimState::Reconnecting { retry_tick: tick + 5 };
                        continue;
                    }

                    // Movement update
                    let delta_dist = self.rng.next_f32() * 1.5;
                    let angle = self.rng.next_f32() * std::f32::consts::TAU;
                    *x += delta_dist * angle.cos();
                    *y += delta_dist * angle.sin();
                    *heading = angle;

                    // Dimension change check
                    if self.config.dimension_count > 1 && (self.rng.next_bounded(100)) == 0 {
                        *dimension = (self.rng.next_bounded(self.config.dimension_count as u64)) as u32;
                    }

                    // Event transmission with packet loss check
                    client.events_sent += 1;
                    let roll_loss = (self.rng.next_bounded(1000)) as u32;
                    if roll_loss < self.config.packet_loss_permille {
                        client.packets_dropped += 1;
                    } else {
                        client.events_received += 1;
                    }
                }
                SimState::Reconnecting { retry_tick } => {
                    if tick >= retry_tick {
                        client.reconnects_completed += 1;
                        client.state = SimState::Handshake { attempt: 1 };
                    }
                }
            }
        }
    }

    /// Compute summary metrics for the current cluster state.
    pub fn metrics(&self) -> ClusterMetrics {
        let mut in_world = 0;
        let mut in_queue = 0;
        let mut reconnecting = 0;
        let mut sent = 0;
        let mut recv = 0;
        let mut dropped = 0;

        for c in &self.clients {
            sent += c.events_sent;
            recv += c.events_received;
            dropped += c.packets_dropped;
            match c.state {
                SimState::InWorld { .. } => in_world += 1,
                SimState::InQueue { .. } => in_queue += 1,
                SimState::Reconnecting { .. } => reconnecting += 1,
                _ => {}
            }
        }

        ClusterMetrics {
            current_tick: self.current_tick,
            total_events_sent: sent,
            total_events_received: recv,
            total_packets_dropped: dropped,
            clients_in_world: in_world,
            clients_in_queue: in_queue,
            clients_reconnecting: reconnecting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_reproducibility() {
        let config = ScenarioConfig {
            client_count: 5,
            seed: 424242,
            packet_loss_permille: 100, // 10%
            disconnect_permille: 10,   // 1%
            queue_depth: 2,
            manifest_resource_count: 3,
            dimension_count: 2,
        };

        let mut sim1 = SimulationCluster::new(config.clone());
        let mut sim2 = SimulationCluster::new(config);

        for _ in 0..50 {
            sim1.step();
            sim2.step();
        }

        let m1 = sim1.metrics();
        let m2 = sim2.metrics();

        assert_eq!(m1, m2);
        for i in 0..5 {
            assert_eq!(sim1.clients()[i].state, sim2.clients()[i].state);
            assert_eq!(sim1.clients()[i].events_sent, sim2.clients()[i].events_sent);
            assert_eq!(sim1.clients()[i].packets_dropped, sim2.clients()[i].packets_dropped);
        }
    }

    #[test]
    fn handshake_to_world_lifecycle() {
        let config = ScenarioConfig { client_count: 1, seed: 123, manifest_resource_count: 2, ..Default::default() };
        let mut sim = SimulationCluster::new(config);

        // Tick 0: Disconnected
        assert_eq!(sim.clients()[0].state, SimState::Disconnected);

        // Tick 1: Handshake 1
        sim.step();
        assert_eq!(sim.clients()[0].state, SimState::Handshake { attempt: 1 });

        // Tick 2: Handshake 2
        sim.step();
        assert_eq!(sim.clients()[0].state, SimState::Handshake { attempt: 2 });

        // Tick 3: ResourceNegotiation (loaded 0/2)
        sim.step();
        assert_eq!(sim.clients()[0].state, SimState::ResourceNegotiation { total: 2, loaded: 0 });

        // Tick 4: ResourceNegotiation (loaded 1/2)
        sim.step();
        assert_eq!(sim.clients()[0].state, SimState::ResourceNegotiation { total: 2, loaded: 1 });

        // Tick 5: InWorld
        sim.step();
        assert!(sim.clients()[0].is_in_world());
    }

    #[test]
    fn movement_updates_position() {
        let config = ScenarioConfig { client_count: 1, seed: 777, manifest_resource_count: 1, ..Default::default() };
        let mut sim = SimulationCluster::new(config);

        // Run until in world
        for _ in 0..10 {
            sim.step();
        }
        assert!(sim.clients()[0].is_in_world());

        let (x1, y1) = match sim.clients()[0].state {
            SimState::InWorld { x, y, .. } => (x, y),
            _ => unreachable!(),
        };

        // Advance 5 more ticks
        for _ in 0..5 {
            sim.step();
        }

        let (x2, y2) = match sim.clients()[0].state {
            SimState::InWorld { x, y, .. } => (x, y),
            _ => unreachable!(),
        };

        assert!(x1 != x2 || y1 != y2, "position should update during walk: ({x1},{y1}) vs ({x2},{y2})");
    }

    #[test]
    fn packet_loss_injection() {
        let config = ScenarioConfig {
            client_count: 2,
            seed: 999,
            packet_loss_permille: 500, // 50% loss
            manifest_resource_count: 1,
            ..Default::default()
        };
        let mut sim = SimulationCluster::new(config);

        for _ in 0..30 {
            sim.step();
        }

        let m = sim.metrics();
        assert!(m.total_packets_dropped > 0, "expected packet loss with 50% loss rate");
        assert_eq!(m.total_events_sent, m.total_events_received + m.total_packets_dropped);
    }

    #[test]
    fn disconnect_and_reconnect() {
        let config = ScenarioConfig {
            client_count: 1,
            seed: 888,
            disconnect_permille: 0,
            manifest_resource_count: 1,
            ..Default::default()
        };
        let mut sim = SimulationCluster::new(config);

        // Reach in-world
        for _ in 0..5 {
            sim.step();
        }
        assert!(sim.clients()[0].is_in_world());

        // Now inject disconnect
        sim.config.disconnect_permille = 1000;
        sim.step();
        assert!(matches!(sim.clients()[0].state, SimState::Reconnecting { .. }));

        // Wait for retry (5 ticks)
        for _ in 0..6 {
            sim.step();
        }

        assert!(sim.clients()[0].reconnects_completed >= 1);
    }

    #[test]
    fn queue_backlog_drain() {
        let config = ScenarioConfig {
            client_count: 3,
            seed: 111,
            queue_depth: 3,
            manifest_resource_count: 1,
            ..Default::default()
        };
        let mut sim = SimulationCluster::new(config);

        // Advance a few steps to queue clients
        for _ in 0..5 {
            sim.step();
        }

        // Keep stepping; queue should eventually drain all clients into world
        for _ in 0..30 {
            sim.step();
        }

        let m = sim.metrics();
        assert_eq!(m.clients_in_queue, 0, "all clients should leave queue");
        assert_eq!(m.clients_in_world, 3, "all 3 clients should reach world");
    }

    #[test]
    fn dimension_interest_shift() {
        let config = ScenarioConfig {
            client_count: 4,
            seed: 555,
            dimension_count: 3,
            manifest_resource_count: 1,
            ..Default::default()
        };
        let mut sim = SimulationCluster::new(config);

        for _ in 0..200 {
            sim.step();
        }

        // Check if dimension values in range 0..3
        for c in sim.clients() {
            if let SimState::InWorld { dimension, .. } = c.state {
                assert!(dimension < 3);
            }
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let c1 = ScenarioConfig { seed: 1, client_count: 2, ..Default::default() };
        let c2 = ScenarioConfig { seed: 2, client_count: 2, ..Default::default() };

        let mut sim1 = SimulationCluster::new(c1);
        let mut sim2 = SimulationCluster::new(c2);

        for _ in 0..25 {
            sim1.step();
            sim2.step();
        }

        assert_ne!(sim1.clients()[0].state, sim2.clients()[0].state);
    }

    #[test]
    fn cluster_multi_client_scale() {
        let config = ScenarioConfig {
            client_count: 25,
            seed: 42,
            packet_loss_permille: 50,
            disconnect_permille: 5,
            manifest_resource_count: 2,
            dimension_count: 4,
            ..Default::default()
        };
        let mut sim = SimulationCluster::new(config);

        for _ in 0..100 {
            sim.step();
        }

        let m = sim.metrics();
        assert_eq!(m.current_tick, 100);
        assert!(m.total_events_sent > 100);
    }

    #[test]
    fn event_accounting_monotonically_increases() {
        let config = ScenarioConfig { client_count: 2, seed: 31415, manifest_resource_count: 1, ..Default::default() };
        let mut sim = SimulationCluster::new(config);

        for _ in 0..10 {
            sim.step();
        }
        let m1 = sim.metrics();

        for _ in 0..10 {
            sim.step();
        }
        let m2 = sim.metrics();

        assert!(m2.total_events_sent > m1.total_events_sent);
        assert!(m2.total_events_received > m1.total_events_received);
    }
}
