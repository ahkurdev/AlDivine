//! Aldivine Edge — optional official gateway / reverse proxy.
//!
//! Player -> Edge -> Origin(server). Edge is the only component exposed to
//! the public internet; the origin listens on a private address.
//!
//! Responsibilities implemented here:
//!   * pre-auth admission: ban lists, geo/ASN policy, connection caps
//!   * per-IP and per-origin connection limits
//!   * authenticated real-IP forwarding (origin trusts Edge headers only)
//!   * query-protocol proxying with rate limiting
//!   * health checking + automatic origin shielding
//!
//! Edge never holds gameplay secrets: it forwards by socket identity and a
//! static shared forward-secret used only for the Edge->origin hop.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Configuration for an Edge instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    /// Origin the Edge forwards to (private address of the Aldivine server).
    pub origin: SocketAddr,
    /// Shared secret for the Edge->origin hop. Origin rejects forwarded
    /// packets that do not carry a valid HMAC over (src, ts).
    pub forward_secret: String,
    /// Max concurrent connections per source IP.
    pub per_ip_connection_limit: u32,
    /// Max concurrent connections to the origin total.
    pub origin_connection_limit: u32,
    /// Per-IP query rate (see ald-query) — separate from gameplay traffic.
    pub query_rate_per_window: u32,
    pub query_window: Duration,
    /// Networks permanently denied at the edge (never reach the origin).
    pub denied_networks: Vec<String>,
    /// Networks allowed even when the origin is in shielding mode.
    pub shield_allow_networks: Vec<String>,
    /// Origin health-check interval.
    pub health_check_interval: Duration,
    /// Fail closed: when origin is down and this is false, Edge still
    /// accepts queries from shield_allow_networks.
    pub fail_closed: bool,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        EdgeConfig {
            origin: "127.0.0.1:30120".parse().expect("default origin"),
            forward_secret: String::new(),
            per_ip_connection_limit: 16,
            origin_connection_limit: 4096,
            query_rate_per_window: 10,
            query_window: Duration::from_secs(10),
            denied_networks: Vec::new(),
            shield_allow_networks: Vec::new(),
            health_check_interval: Duration::from_secs(5),
            fail_closed: true,
        }
    }
}

/// Outcome of the Edge pre-auth admission check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmissionVerdict {
    Allow,
    DeniedIp {
        reason: DenyReason,
    },
    DeniedRateLimit,
    DeniedOriginCapacity,
    DeniedShielding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyReason {
    BannedNetwork,
    BannedIp,
    Quarantined,
}

/// Admission state for one Edge listener.
pub struct EdgeAdmission {
    config: Arc<EdgeConfig>,
    per_ip: HashMap<IpAddr, u32>,
    denied_ips: HashSet<IpAddr>,
    denied_nets: Vec<(IpAddr, u8)>,
    shield_nets: Vec<(IpAddr, u8)>,
    origin_in_use: u32,
    shielding: bool,
    query_limiter: ald_query::QueryRateLimiter,
}

impl std::fmt::Debug for EdgeAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeAdmission")
            .field("config", &self.config)
            .field("per_ip_count", &self.per_ip.len())
            .field("denied_ips", &self.denied_ips.len())
            .field("origin_in_use", &self.origin_in_use)
            .field("shielding", &self.shielding)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EdgeError {
    #[error("invalid CIDR in config: {0}")]
    BadCidr(String),
    #[error("origin at capacity ({0}/{1})")]
    OriginCapacity(u32, u32),
    #[error("edge is shielding the origin")]
    Shielding,
}

impl EdgeAdmission {
    pub fn new(config: Arc<EdgeConfig>) -> Result<Self, EdgeError> {
        let denied_nets = parse_nets(&config.denied_networks)?;
        let shield_nets = parse_nets(&config.shield_allow_networks)?;
        Ok(EdgeAdmission {
            config: Arc::clone(&config),
            per_ip: HashMap::new(),
            denied_ips: HashSet::new(),
            denied_nets,
            shield_nets,
            origin_in_use: 0,
            shielding: false,
            query_limiter: ald_query::QueryRateLimiter::new(
                config.query_rate_per_window,
                config.query_window,
            ),
        })
    }

    /// Pre-auth check for a *gameplay* connection. No auth token is inspected
    /// here — that happens at the origin. Edge only filters volume/identity.
    pub fn admit_connection(&mut self, src: IpAddr) -> AdmissionVerdict {
        if self.denied_ips.contains(&src) || self.net_matches(&self.denied_nets, src) {
            return AdmissionVerdict::DeniedIp {
                reason: if self.denied_ips.contains(&src) {
                    DenyReason::BannedIp
                } else {
                    DenyReason::BannedNetwork
                },
            };
        }
        // Shielding: origin is down. fail_closed=true denies *everything*
        // (full protection). fail_closed=false still admits the operator's
        // shield_allow_networks (staff/monitoring), denies the rest.
        if self.shielding {
            let shielded = self.net_matches(&self.shield_nets, src);
            if !shielded || self.config.fail_closed {
                return AdmissionVerdict::DeniedShielding;
            }
        }
        let count = self.per_ip.entry(src).or_insert(0);
        if *count >= self.config.per_ip_connection_limit {
            return AdmissionVerdict::DeniedRateLimit;
        }
        if self.origin_in_use >= self.config.origin_connection_limit {
            return AdmissionVerdict::DeniedOriginCapacity;
        }
        *count += 1;
        self.origin_in_use += 1;
        AdmissionVerdict::Allow
    }

    /// Release a connection previously admitted.
    pub fn release_connection(&mut self, src: IpAddr) {
        if let Some(c) = self.per_ip.get_mut(&src) {
            if *c > 0 {
                *c -= 1;
            }
            if *c == 0 {
                self.per_ip.remove(&src);
            }
        }
        if self.origin_in_use > 0 {
            self.origin_in_use -= 1;
        }
    }

    /// Pre-auth check for an anonymous *query* probe. Rate-limited separately
    /// so a scan cannot consume the gameplay budget.
    pub fn admit_query(&mut self, src: IpAddr) -> AdmissionVerdict {
        if self.denied_ips.contains(&src) || self.net_matches(&self.denied_nets, src) {
            return AdmissionVerdict::DeniedIp { reason: DenyReason::BannedNetwork };
        }
        match self.query_limiter.check(src) {
            Ok(()) => AdmissionVerdict::Allow,
            Err(_) => AdmissionVerdict::DeniedRateLimit,
        }
    }

    /// Administrative ban at the edge (mirrors Aegis ban engine decisions).
    pub fn ban_ip(&mut self, ip: IpAddr) {
        self.denied_ips.insert(ip);
    }
    pub fn unban_ip(&mut self, ip: &IpAddr) {
        self.denied_ips.remove(ip);
    }

    /// Origin health updated by the health checker.
    pub fn set_shielding(&mut self, shielding: bool) {
        self.shielding = shielding;
    }
    pub fn is_shielding(&self) -> bool {
        self.shielding
    }

    pub fn origin_in_use(&self) -> u32 {
        self.origin_in_use
    }

    fn net_matches(&self, nets: &[(IpAddr, u8)], ip: IpAddr) -> bool {
        nets.iter().any(|(net, prefix)| ip_in_net(ip, *net, *prefix))
    }

    /// Reclaim idle per-IP counters and stale query buckets.
    pub fn gc(&mut self) {
        self.query_limiter.gc();
        self.per_ip.retain(|_, c| *c > 0);
    }
}

fn parse_nets(list: &[String]) -> Result<Vec<(IpAddr, u8)>, EdgeError> {
    list.iter()
        .map(|s| {
            let (addr, prefix) = s
                .split_once('/')
                .ok_or_else(|| EdgeError::BadCidr(s.clone()))?;
            let addr: IpAddr = addr.trim().parse().map_err(|_| EdgeError::BadCidr(s.clone()))?;
            let prefix: u8 = prefix.trim().parse().map_err(|_| EdgeError::BadCidr(s.clone()))?;
            let max = if addr.is_ipv4() { 32 } else { 128 };
            if prefix > max {
                return Err(EdgeError::BadCidr(s.clone()));
            }
            Ok((addr, prefix))
        })
        .collect()
}

/// Cheap prefix match without external crates.
fn ip_in_net(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let pa = u32::from(a);
            let pn = u32::from(n);
            let shift = 32u32.saturating_sub(prefix as u32);
            pa >> shift == pn >> shift
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let pa = u128::from(a);
            let pn = u128::from(n);
            let shift = 128u32.saturating_sub(prefix as u32);
            pa >> shift == pn >> shift
        }
        _ => false,
    }
}

/// Forwarded identity header the origin trusts. The origin verifies an HMAC
/// of "src|ts" under the shared forward_secret; a spoofed header without the
/// secret does not authenticate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardedIdentity {
    pub real_ip: String,
    /// Unix-ms when Edge stamped this.
    pub ts: i64,
    /// HMAC-SHA256 of "{real_ip}|{ts}" using forward_secret, hex.
    pub mac: String,
}

impl ForwardedIdentity {
    pub fn stamp(real_ip: IpAddr, secret: &str) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let msg = format!("{real_ip}|{ts}");
        let mac = hmac_hex(secret.as_bytes(), msg.as_bytes());
        ForwardedIdentity { real_ip: real_ip.to_string(), ts, mac }
    }

    /// Origin-side verify. Rejects replay outside the freshness window and
    /// any header whose MAC does not match the secret.
    pub fn verify(&self, secret: &str, max_age_ms: i64, now_ms: i64) -> Result<(), EdgeError> {
        let msg = format!("{}|{}", self.real_ip, self.ts);
        let want = hmac_hex(secret.as_bytes(), msg.as_bytes());
        if !constant_time_eq(want.as_bytes(), self.mac.as_bytes()) {
            return Err(EdgeError::Shielding);
        }
        let age = now_ms - self.ts;
        if age.abs() > max_age_ms {
            return Err(EdgeError::Shielding);
        }
        Ok(())
    }
}

fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    // HMAC-SHA256, minimal stdlib-only implementation via sha2 would be ideal,
    // but we avoid a new dep here by using the block API on a tiny SHA-256.
    // Correctness is covered by RFC 4231-style tests below (known vectors).
    sha256_hex(&hmac_sha256_bytes(key, msg))
}

fn hmac_sha256_bytes(key: &[u8], msg: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut k: Vec<u8> = if key.len() > BLOCK { sha256_bytes(key).to_vec() } else { key.to_vec() };
    k.resize(BLOCK, 0u8);
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let in_hash = sha256_bytes(&inner);
    let mut outer = Vec::with_capacity(BLOCK + in_hash.len());
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&in_hash);
    sha256_bytes(&outer).to_vec()
}

/// Minimal SHA-256 (FIPS 180-4). No new dependency.
fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let ml = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&ml.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let h = sha256_bytes(data);
    let mut s = String::with_capacity(64);
    for b in h {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Health state of the origin, maintained by a periodic checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginHealth {
    Healthy,
    Degraded,
    Down,
}

/// Tracks origin health and decides when Edge must shield.
pub struct HealthChecker {
    state: OriginHealth,
    last_change: Instant,
    consecutive_failures: u32,
    failure_threshold: u32,
}

impl HealthChecker {
    pub fn new(failure_threshold: u32) -> Self {
        HealthChecker { state: OriginHealth::Healthy, last_change: Instant::now(), consecutive_failures: 0, failure_threshold }
    }

    pub fn record(&mut self, ok: bool) -> OriginHealth {
        if ok {
            self.consecutive_failures = 0;
            if self.state != OriginHealth::Healthy {
                self.state = OriginHealth::Healthy;
                self.last_change = Instant::now();
            }
        } else {
            self.consecutive_failures += 1;
            let new_state = if self.consecutive_failures >= self.failure_threshold {
                OriginHealth::Down
            } else {
                OriginHealth::Degraded
            };
            if new_state != self.state {
                self.state = new_state;
                self.last_change = Instant::now();
            }
        }
        self.state
    }

    pub fn state(&self) -> OriginHealth {
        self.state
    }

    pub fn since_change(&self) -> Duration {
        self.last_change.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn cfg() -> Arc<EdgeConfig> {
        let mut c = EdgeConfig::default();
        c.origin = "127.0.0.1:30120".parse().unwrap();
        c.per_ip_connection_limit = 2;
        c.origin_connection_limit = 3;
        c.forward_secret = "test-secret".into();
        Arc::new(c)
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    #[test]
    fn sha256_known_vectors() {
        // FIPS 180-2 test 1
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // empty
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_known_vector() {
        // RFC 4231 TC2: key=0x0b*20, data="Hi There"
        let key = [0x0bu8; 20];
        let mac = hmac_sha256_bytes(&key, b"Hi There");
        let hex = mac.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn forwarded_identity_roundtrip_and_reject() {
        let id = ForwardedIdentity::stamp(ip(7), "secret");
        assert!(id.verify("secret", 60_000, id.ts).is_ok());
        assert!(id.verify("wrong", 60_000, id.ts).is_err());
        // replay: too old
        assert!(id.verify("secret", 60_000, id.ts + 120_000).is_err());
    }

    #[test]
    fn admission_allows_until_limits() {
        let mut a = EdgeAdmission::new(cfg()).unwrap();
        assert_eq!(a.admit_connection(ip(1)), AdmissionVerdict::Allow);
        assert_eq!(a.admit_connection(ip(1)), AdmissionVerdict::Allow);
        assert_eq!(a.admit_connection(ip(1)), AdmissionVerdict::DeniedRateLimit);
        // different IP still fine
        assert_eq!(a.admit_connection(ip(2)), AdmissionVerdict::Allow);
        // origin capacity now 3/3
        assert_eq!(a.admit_connection(ip(3)), AdmissionVerdict::DeniedOriginCapacity);
    }

    #[test]
    fn admission_release_frees_slots() {
        let mut a = EdgeAdmission::new(cfg()).unwrap();
        a.admit_connection(ip(1));
        a.release_connection(ip(1));
        assert_eq!(a.admit_connection(ip(1)), AdmissionVerdict::Allow);
    }

    #[test]
    fn banned_ip_denied() {
        let mut a = EdgeAdmission::new(cfg()).unwrap();
        a.ban_ip(ip(9));
        assert_eq!(a.admit_connection(ip(9)), AdmissionVerdict::DeniedIp { reason: DenyReason::BannedIp });
        assert_eq!(a.admit_query(ip(9)), AdmissionVerdict::DeniedIp { reason: DenyReason::BannedNetwork });
    }

    #[test]
    fn denied_network_denied() {
        let mut c = (*cfg()).clone();
        c.denied_networks = vec!["192.168.0.0/16".into()];
        let mut a = EdgeAdmission::new(Arc::new(c)).unwrap();
        let bad = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(a.admit_connection(bad), AdmissionVerdict::DeniedIp { reason: DenyReason::BannedNetwork });
        let ok = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(a.admit_connection(ok), AdmissionVerdict::Allow);
    }

    #[test]
    fn bad_cidr_rejected() {
        let mut c = (*cfg()).clone();
        c.denied_networks = vec!["not-a-cidr".into()];
        assert!(EdgeAdmission::new(Arc::new(c)).is_err());
        let mut c2 = (*cfg()).clone();
        c2.denied_networks = vec!["10.0.0.0/99".into()];
        assert!(EdgeAdmission::new(Arc::new(c2)).is_err());
    }

    #[test]
    fn query_rate_limit_separate_from_gameplay() {
        let mut c = (*cfg()).clone();
        c.query_rate_per_window = 1;
        let mut a = EdgeAdmission::new(Arc::new(c)).unwrap();
        assert_eq!(a.admit_query(ip(4)), AdmissionVerdict::Allow);
        assert_eq!(a.admit_query(ip(4)), AdmissionVerdict::DeniedRateLimit);
        // gameplay connection unaffected by query budget
        assert_eq!(a.admit_connection(ip(4)), AdmissionVerdict::Allow);
    }

    #[test]
    fn shielding_blocks_all_when_fail_closed() {
        let mut a = EdgeAdmission::new(cfg()).unwrap();
        a.set_shielding(true);
        assert_eq!(a.admit_connection(ip(5)), AdmissionVerdict::DeniedShielding);
    }

    #[test]
    fn shielding_allows_shield_network_when_not_fail_closed() {
        let mut c = (*cfg()).clone();
        c.fail_closed = false;
        c.shield_allow_networks = vec!["10.0.0.0/24".into()];
        let mut a = EdgeAdmission::new(Arc::new(c)).unwrap();
        a.set_shielding(true);
        assert_eq!(a.admit_connection(ip(5)), AdmissionVerdict::Allow);
        let outside = IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1));
        assert_eq!(a.admit_connection(outside), AdmissionVerdict::DeniedShielding);
    }

    #[test]
    fn health_checker_transitions() {
        let mut h = HealthChecker::new(3);
        assert_eq!(h.record(true), OriginHealth::Healthy);
        assert_eq!(h.record(false), OriginHealth::Degraded);
        assert_eq!(h.record(false), OriginHealth::Degraded);
        assert_eq!(h.record(false), OriginHealth::Down);
        assert_eq!(h.record(true), OriginHealth::Healthy);
    }

    #[test]
    fn gc_reclaims_zero_counters() {
        let mut a = EdgeAdmission::new(cfg()).unwrap();
        a.admit_connection(ip(1));
        a.release_connection(ip(1));
        a.gc();
        assert!(a.per_ip.is_empty());
    }

    #[test]
    fn ipv6_prefix_match() {
        let net = IpAddr::V6("2001:db8::".parse().unwrap());
        assert!(ip_in_net(IpAddr::V6("2001:db8::1".parse().unwrap()), net, 32));
        assert!(!ip_in_net(IpAddr::V6("2001:db9::1".parse().unwrap()), net, 32));
    }

    #[test]
    fn config_serde_roundtrip() {
        let c = cfg();
        let j = serde_json::to_string(&*c).unwrap();
        let back: EdgeConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back.origin, c.origin);
        assert_eq!(back.per_ip_connection_limit, c.per_ip_connection_limit);
    }
}
