//! Aldivine Time Synchronization.
//!
//! Server-authoritative clock. Clients estimate offset via NTP-style
//! round-trip exchange; the server never trusts client timestamps.
//!
//! Model:
//!   t_client_now = t_server_now + offset
//!   offset estimated from (t1 - t0 - rtt/2) using request/reply timestamps.
//!
//! Security: a client clock offset is *advisory* for interpolation only.
//! Server-side history bounds (lag compensation) always use the server's own
//! monotonic clock, never a client-supplied value.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A server time sample exchanged with a client.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeSample {
    /// Client send time (their clock), ms since their epoch.
    pub t0: i64,
    /// Server receive time (server clock), ms since UNIX_EPOCH.
    pub t1: i64,
    /// Server send time (server clock), ms since UNIX_EPOCH.
    pub t2: i64,
}

impl TimeSample {
    /// Build the server-side reply half of an exchange.
    pub fn server_reply(t0: i64) -> Self {
        let now = server_now_millis();
        TimeSample { t0, t1: now, t2: now }
    }
}

/// Result of a client-side time sync round.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SyncResult {
    /// Estimated offset: client_now = server_now + offset (ms).
    pub offset_ms: i64,
    /// Round-trip time (ms).
    pub rtt_ms: i64,
    /// Best (lowest-rtt) sample's observed jitter (ms).
    pub jitter_ms: i64,
}

impl SyncResult {
    /// Client-side: compute offset/rtt from a completed round.
    /// t3 = client receive time (their clock), ms.
    pub fn from_sample(s: &TimeSample, t3: i64) -> Self {
        let rtt = (t3 - s.t0) - (s.t2 - s.t1);
        let rtt = rtt.max(0);
        // Classic NTP offset: ((t1-t0)+(t2-t3))/2
        let offset = ((s.t1 - s.t0) + (s.t2 - t3)) / 2;
        SyncResult { offset_ms: offset, rtt_ms: rtt, jitter_ms: 0 }
    }
}

/// Server-authoritative clock used for history bounds and tick epochs.
/// Monotonic for measurement; wall-clock for timestamps that must survive
/// restart (persistent state uses the DB clock instead).
#[derive(Debug, Clone, Copy)]
pub struct ServerClock {
    /// Instant at which `epoch` was captured.
    anchor: Instant,
    /// Wall-clock ms at `anchor`.
    epoch: i64,
}

impl ServerClock {
    pub fn new() -> Self {
        ServerClock { anchor: Instant::now(), epoch: server_now_millis() }
    }

    /// Tick epoch: ms elapsed since this server started.
    pub fn tick_ms(&self) -> u64 {
        self.anchor.elapsed().as_millis() as u64
    }

    /// Current wall clock ms (extrapolated from anchor; immune to wall-clock
    /// jumps during a session).
    pub fn now_ms(&self) -> i64 {
        self.epoch + self.anchor.elapsed().as_millis() as i64
    }

    /// Monotonic instant for interval measurement.
    pub fn instant(&self) -> Instant {
        Instant::now()
    }
}

impl Default for ServerClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Client-side clock-estimate tracker. Keeps the best N samples by RTT and
/// derives offset + jitter. Older samples decay out.
#[derive(Debug)]
pub struct ClockEstimator {
    samples: Vec<SyncResult>,
    capacity: usize,
}

impl ClockEstimator {
    pub fn new(capacity: usize) -> Self {
        ClockEstimator { samples: Vec::new(), capacity: capacity.max(1) }
    }

    pub fn push(&mut self, r: SyncResult) {
        if self.samples.len() >= self.capacity {
            // evict highest-rtt sample
            if let Some(worst) = self.samples.iter().enumerate().max_by_key(|(_, s)| s.rtt_ms).map(|(i, _)| i) {
                self.samples.remove(worst);
            }
        }
        self.samples.push(r);
    }

    /// Offset of the lowest-rtt sample (most accurate single estimate).
    pub fn offset_ms(&self) -> Option<i64> {
        self.samples.iter().min_by_key(|s| s.rtt_ms).map(|s| s.offset_ms)
    }

    /// Jitter: max spread of offsets across samples (ms).
    pub fn jitter_ms(&self) -> i64 {
        match self.samples.len() {
            0 | 1 => 0,
            _ => {
                let mut min = i64::MAX;
                let mut max = i64::MIN;
                for s in &self.samples {
                    min = min.min(s.offset_ms);
                    max = max.max(s.offset_ms);
                }
                (max - min).abs()
            }
        }
    }

    /// Smoothed RTT (mean of samples).
    pub fn rtt_ms(&self) -> i64 {
        if self.samples.is_empty() {
            return 0;
        }
        self.samples.iter().map(|s| s.rtt_ms).sum::<i64>() / self.samples.len() as i64
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// Interpolation delay bound: how far behind the server clock a client should
/// render remote entities. Grows with jitter and RTT, clamped to a sane max
/// so a bad connection cannot stall rendering indefinitely.
pub fn interpolation_delay_ms(rtt_ms: i64, jitter_ms: i64) -> i64 {
    const MIN_DELAY: i64 = 60; // ~2 ticks @ 30Hz, hide reorder
    const MAX_DELAY: i64 = 400;
    let jitter_guard = jitter_ms.max(0);
    let base = (rtt_ms / 2).max(0);
    (MIN_DELAY + base + jitter_guard).min(MAX_DELAY)
}

/// Bound a client-supplied timestamp against the server clock.
/// Returns Err if the claim is outside [now - MAX_LAG, now + MAX_LEAD],
/// i.e. replayable/forged history is rejected rather than trusted.
pub fn validate_client_time_claim(
    server_now: i64,
    client_claim: i64,
    allowed_lag_ms: i64,
) -> Result<(), TimeSyncError> {
    let lag = server_now - client_claim;
    if lag > allowed_lag_ms {
        return Err(TimeSyncError::StaleClaim { lag_ms: lag, allowed_lag_ms });
    }
    if lag < -MAX_LEAD_MS {
        return Err(TimeSyncError::FutureClaim { lead_ms: -lag });
    }
    Ok(())
}

/// Maximum a client may claim to be *ahead* of the server. Legit clients with
/// a fast clock + negative offset may be slightly ahead, but never far.
pub const MAX_LEAD_MS: i64 = 5000;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TimeSyncError {
    #[error("stale client time claim: {lag_ms}ms behind server (allowed {allowed_lag_ms}ms)")]
    StaleClaim { lag_ms: i64, allowed_lag_ms: i64 },
    #[error("future client time claim: {lead_ms}ms ahead of server")]
    FutureClaim { lead_ms: i64 },
    #[error("no clock samples yet")]
    NoSamples,
}

/// Milliseconds since UNIX_EPOCH on the server wall clock.
pub fn server_now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Milliseconds since process start (monotonic, for tick epochs).
pub fn tick_now_millis() -> u64 {
    // Cheap monotonic: anchor captured once via lazy static.
    static ANCHOR: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let a = ANCHOR.get_or_init(Instant::now);
    a.elapsed().as_millis() as u64
}

/// Snapshot of time-sync diagnostics surfaced to Aegis/F8 console.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSyncDiagnostics {
    pub tick_epoch_ms: u64,
    pub server_now_ms: i64,
    pub rtt_p50_ms: i64,
    pub jitter_ms: i64,
    pub interpolation_delay_ms: i64,
    pub sample_count: usize,
}

impl TimeSyncDiagnostics {
    pub fn from_estimator(clock: &ServerClock, est: &ClockEstimator) -> Self {
        let rtt = est.rtt_ms();
        let jitter = est.jitter_ms();
        TimeSyncDiagnostics {
            tick_epoch_ms: clock.tick_ms(),
            server_now_ms: clock.now_ms(),
            rtt_p50_ms: rtt,
            jitter_ms: jitter,
            interpolation_delay_ms: interpolation_delay_ms(rtt, jitter),
            sample_count: est.len(),
        }
    }
}

/// Useful duration constants for tick scheduling.
pub mod durations {
    use std::time::Duration;
    pub const TICK_30HZ: Duration = Duration::from_millis(33);
    pub const TICK_60HZ: Duration = Duration::from_millis(16);
    pub const SYNC_INTERVAL: Duration = Duration::from_secs(5);
    pub const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(30);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn roundtrip_offsets_cancel() {
        // Symmetric-ish delay: offset must be exactly the true offset.
        // t0 client send; server receives 50ms later (server clock); server
        // holds 10ms; client receives 110ms after send -> rtt = 110 - 10 = 100.
        let true_offset = 1234;
        let t0 = 1_000_000;
        let t1 = t0 + true_offset + 50;
        let t2 = t1 + 10;
        let t3 = t0 + 110;
        let s = TimeSample { t0, t1, t2 };
        let r = SyncResult::from_sample(&s, t3);
        assert_eq!(r.offset_ms, true_offset);
        assert_eq!(r.rtt_ms, 100);
    }

    #[test]
    fn asymmetric_delay_biases_toward_half_rtt() {
        // Asymmetric path: offset error is bounded by the asymmetry, never more
        // than the full rtt. This is why we keep the lowest-rtt sample.
        let t0 = 0;
        let t1 = 1000; // 1000ms out
        let t2 = 1010;
        let t3 = 1100; // 90ms back; rtt = 1100 - 10 = 1090
        let s = TimeSample { t0, t1, t2 };
        let r = SyncResult::from_sample(&s, t3);
        assert_eq!(r.rtt_ms, 1090);
        let est = ((t1 - t0) + (t2 - t3)) / 2;
        assert_eq!(r.offset_ms, est);
        let true_offset = 1000 - 1090 / 2; // if symmetric
        assert!((r.offset_ms - true_offset).abs() <= 1090);
    }

    #[test]
    fn clock_estimator_prefers_low_rtt() {
        let mut e = ClockEstimator::new(3);
        e.push(SyncResult { offset_ms: 100, rtt_ms: 300, jitter_ms: 0 });
        e.push(SyncResult { offset_ms: 120, rtt_ms: 40, jitter_ms: 0 });
        e.push(SyncResult { offset_ms: 110, rtt_ms: 500, jitter_ms: 0 });
        assert_eq!(e.offset_ms(), Some(120));
        assert_eq!(e.len(), 3);
        // capacity evicts worst rtt on push
        e.push(SyncResult { offset_ms: 130, rtt_ms: 10, jitter_ms: 0 });
        assert_eq!(e.len(), 3);
        assert_eq!(e.offset_ms(), Some(130));
        assert!(e.samples.iter().all(|s| s.rtt_ms < 500));
    }

    #[test]
    fn jitter_is_spread() {
        let mut e = ClockEstimator::new(4);
        e.push(SyncResult { offset_ms: 100, rtt_ms: 50, jitter_ms: 0 });
        assert_eq!(e.jitter_ms(), 0);
        e.push(SyncResult { offset_ms: 140, rtt_ms: 60, jitter_ms: 0 });
        assert_eq!(e.jitter_ms(), 40);
    }

    #[test]
    fn interpolation_delay_clamps() {
        assert!(interpolation_delay_ms(0, 0) >= 60);
        assert!(interpolation_delay_ms(2000, 3000) <= 400);
        assert!(interpolation_delay_ms(100, 50) >= 60 + 50 + 50);
    }

    #[test]
    fn stale_claim_rejected() {
        assert!(validate_client_time_claim(1_000_000, 1_000_000 - 5000, 1000).is_err());
        assert!(validate_client_time_claim(1_000_000, 1_000_000 + 999_999, 1000).is_err());
        assert!(validate_client_time_claim(1_000_000, 1_000_000 - 500, 1000).is_ok());
    }

    #[test]
    fn server_clock_is_monotonic_and_consistent() {
        let c = ServerClock::new();
        let a = c.tick_ms();
        std::thread::sleep(Duration::from_millis(20));
        let b = c.tick_ms();
        assert!(b > a);
        // now_ms tracks tick epoch within rounding
        assert!((c.now_ms() - c.epoch) >= 19);
    }

    #[test]
    fn tick_now_is_monotonic() {
        let a = tick_now_millis();
        std::thread::sleep(Duration::from_millis(10));
        let b = tick_now_millis();
        assert!(b > a);
    }

    #[test]
    fn diagnostics_compose() {
        let c = ServerClock::new();
        let mut e = ClockEstimator::new(2);
        e.push(SyncResult { offset_ms: 5, rtt_ms: 80, jitter_ms: 0 });
        e.push(SyncResult { offset_ms: 15, rtt_ms: 90, jitter_ms: 0 });
        let d = TimeSyncDiagnostics::from_estimator(&c, &e);
        assert_eq!(d.sample_count, 2);
        assert_eq!(d.rtt_p50_ms, 85);
        assert_eq!(d.jitter_ms, 10);
        assert!(d.interpolation_delay_ms >= 60);
    }

    #[test]
    fn sample_roundtrips_serde() {
        let s = TimeSample { t0: 1, t1: 2, t2: 3 };
        let j = serde_json::to_string(&s).unwrap();
        let back: TimeSample = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}
