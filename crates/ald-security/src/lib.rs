//! Secret redaction + log sanitization. Ensures secrets never reach logs or
//! crash reports. The redaction is tested against known secret shapes.
//! Also provides anti-cheat movement/bounds verification, SSRF protection,
//! DDoS token-bucket rate limiting, and client integrity checks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use thiserror::Error;

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    patterns: Vec<(&'static str, &'static str)>,
}

impl Redactor {
    pub fn new() -> Self {
        Redactor {
            patterns: vec![
                ("access_token", "***REDACTED***"),
                ("refresh_token", "***REDACTED***"),
                ("authorization", "***REDACTED***"),
                ("session_cookie", "***REDACTED***"),
                ("password", "***REDACTED***"),
                ("db_password", "***REDACTED***"),
                ("secret", "***REDACTED***"),
                ("api_key", "***REDACTED***"),
            ],
        }
    }

    /// Replace known secret key occurrences in a log line.
    pub fn redact(&self, line: &str) -> String {
        let mut out = line.to_string();
        for (key, replacement) in &self.patterns {
            // Match key="value" or key: "value" forms.
            let owned;
            let pat = if let Some(stripped) = key.strip_prefix('"') {
                stripped
            } else {
                owned = key.to_string();
                &owned
            };
            let kv = format!("{pat}=\"");
            if let Some(pos) = out.find(&kv) {
                if let Some(end) = out[pos + kv.len()..].find('"') {
                    let start = pos + kv.len();
                    let stop = start + end;
                    out.replace_range(start..stop, replacement);
                }
            }
            let kv2 = format!("{pat}: \"");
            if let Some(pos) = out.find(&kv2) {
                if let Some(end) = out[pos + kv2.len()..].find('"') {
                    let start = pos + kv2.len();
                    let stop = start + end;
                    out.replace_range(start..stop, replacement);
                }
            }
        }
        out
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SsrfError {
    #[error("invalid or unparseable url: {0}")]
    InvalidUrl(String),
    #[error("disallowed scheme: {0}; only http and https are permitted")]
    DisallowedScheme(String),
    #[error("blocked host: {0} resolves to private, loopback, or cloud metadata address")]
    BlockedHost(String),
}

/// SSRF protection filter for webhooks and outbound requests.
pub struct SsrfFilter;

impl SsrfFilter {
    pub fn validate_url(url_str: &str) -> Result<(), SsrfError> {
        if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
            return Err(SsrfError::DisallowedScheme(url_str.to_string()));
        }

        // Extract host
        let after_scheme = if let Some(stripped) = url_str.strip_prefix("https://") {
            stripped
        } else if let Some(stripped) = url_str.strip_prefix("http://") {
            stripped
        } else {
            return Err(SsrfError::InvalidUrl(url_str.to_string()));
        };

        let host_part = after_scheme
            .split(['/', '?', '#', ':'])
            .next()
            .ok_or_else(|| SsrfError::InvalidUrl(url_str.to_string()))?;

        let host_lower = host_part.to_ascii_lowercase();

        // Disallow localhost and common local hostnames
        if host_lower == "localhost" || host_lower.ends_with(".local") || host_lower.ends_with(".internal") {
            return Err(SsrfError::BlockedHost(host_lower));
        }

        // Parse IP directly if supplied
        if let Ok(ip) = host_lower.parse::<IpAddr>() {
            if Self::is_blocked_ip(&ip) {
                return Err(SsrfError::BlockedHost(ip.to_string()));
            }
        }

        Ok(())
    }

    fn is_blocked_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                // 127.0.0.0/8 (loopback)
                if octets[0] == 127 {
                    return true;
                }
                // 10.0.0.0/8 (RFC 1918)
                if octets[0] == 10 {
                    return true;
                }
                // 172.16.0.0/12 (RFC 1918)
                if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                    return true;
                }
                // 192.168.0.0/16 (RFC 1918)
                if octets[0] == 192 && octets[1] == 168 {
                    return true;
                }
                // 169.254.0.0/16 (link-local, cloud metadata 169.254.169.254)
                if octets[0] == 169 && octets[1] == 254 {
                    return true;
                }
                // 0.0.0.0/8
                if octets[0] == 0 {
                    return true;
                }
                false
            }
            IpAddr::V6(ipv6) => ipv6.is_loopback(),
        }
    }
}

/// Anti-cheat violation classification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AntiCheatViolation {
    #[error("teleport/speed violation: moved {distance}m in {dt}s (speed {speed}m/s exceeds max {max_speed}m/s)")]
    SpeedHack { distance: u32, dt: u32, speed: u32, max_speed: u32 },
    #[error("out of bounds violation: ({x}, {y}, {z}) outside valid world area")]
    OutOfBounds { x: i32, y: i32, z: i32 },
    #[error("unauthorized godmode/health change from {current} to {incoming}")]
    IllegalHealthModification { current: u32, incoming: u32 },
}

/// Position snapshot for movement verification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionSample {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub timestamp_ms: u64,
}

/// Server-authoritative anti-cheat physics and telemetry inspector.
pub struct AntiCheatDetector;

impl AntiCheatDetector {
    /// Verify movement does not exceed physical speed bounds (accounting for latency tolerance).
    pub fn verify_movement(
        prev: PositionSample,
        next: PositionSample,
        max_speed_mps: f32,
    ) -> Result<(), AntiCheatViolation> {
        let dt_ms = next.timestamp_ms.saturating_sub(prev.timestamp_ms);
        if dt_ms == 0 {
            return Ok(());
        }
        let dt_sec = (dt_ms as f32) / 1000.0;

        let dx = next.x - prev.x;
        let dy = next.y - prev.y;
        let dz = next.z - prev.z;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        // 1.5x margin of error for lag spikes / sync interpolation
        let allowed_distance = (max_speed_mps * dt_sec) * 1.5 + 5.0;

        if dist > allowed_distance {
            let speed = dist / dt_sec;
            return Err(AntiCheatViolation::SpeedHack {
                distance: dist as u32,
                dt: dt_sec as u32,
                speed: speed as u32,
                max_speed: max_speed_mps as u32,
            });
        }

        Ok(())
    }

    /// Check if coordinates fall within valid GTA V map bounds.
    pub fn verify_world_bounds(x: f32, y: f32, z: f32) -> Result<(), AntiCheatViolation> {
        // Broad GTA V bounding box (-4500..5500 X, -4500..8500 Y, -200..3000 Z)
        if !(-4500.0..=5500.0).contains(&x) || !(-4500.0..=8500.0).contains(&y) || !(-200.0..=3000.0).contains(&z) {
            return Err(AntiCheatViolation::OutOfBounds { x: x as i32, y: y as i32, z: z as i32 });
        }
        Ok(())
    }

    /// Check health modifications. Client cannot heal themselves unless server granted it.
    pub fn verify_health_update(
        current: u32,
        incoming: u32,
        is_server_granted: bool,
    ) -> Result<(), AntiCheatViolation> {
        if incoming > current && !is_server_granted {
            return Err(AntiCheatViolation::IllegalHealthModification { current, incoming });
        }
        Ok(())
    }
}

/// Token bucket rate limiter for DDoS and event spam defense.
#[derive(Debug, Clone)]
pub struct DdosRateLimiter {
    capacity: u32,
    refill_rate_per_sec: u32,
    buckets: HashMap<String, (f32, u64)>, // key -> (tokens, last_update_ms)
}

impl DdosRateLimiter {
    pub fn new(capacity: u32, refill_rate_per_sec: u32) -> Self {
        Self { capacity, refill_rate_per_sec, buckets: HashMap::new() }
    }

    /// Attempt to consume 1 token. Returns true if allowed, false if rate limited.
    pub fn check_allow(&mut self, key: &str, now_ms: u64) -> bool {
        let entry = self.buckets.entry(key.to_string()).or_insert((self.capacity as f32, now_ms));
        let (ref mut tokens, ref mut last_update_ms) = entry;

        // Refill tokens based on elapsed time
        let elapsed_sec = now_ms.saturating_sub(*last_update_ms) as f32 / 1000.0;
        *tokens = (*tokens + elapsed_sec * self.refill_rate_per_sec as f32).min(self.capacity as f32);
        *last_update_ms = now_ms;

        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Client binary integrity verifier.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientIntegrityVerifier {
    allowed_hashes: HashMap<String, String>, // build_id -> sha256_hex
}

impl ClientIntegrityVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_allowed_build(&mut self, build_id: impl Into<String>, hash_hex: impl Into<String>) {
        self.allowed_hashes.insert(build_id.into(), hash_hex.into());
    }

    pub fn verify_client_hash(&self, build_id: &str, client_hash_hex: &str) -> bool {
        if let Some(expected) = self.allowed_hashes.get(build_id) {
            expected.eq_ignore_ascii_case(client_hash_hex)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password() {
        let r = Redactor::new();
        let line = r#"login user=allan password="hunter2pass""#;
        let out = r.redact(line);
        assert!(!out.contains("hunter2pass"));
        assert!(out.contains("***REDACTED***"));
    }

    #[test]
    fn redacts_authorization_header() {
        let r = Redactor::new();
        let line = r#"req authorization="Bearer eyJabc.def.ghi""#;
        let out = r.redact(line);
        assert!(!out.contains("eyJabc"));
    }

    #[test]
    fn leaves_normal_logs() {
        let r = Redactor::new();
        let line = "player joined id=5 tick=120";
        assert_eq!(r.redact(line), line);
    }

    #[test]
    fn ssrf_blocks_localhost_and_loopback() {
        assert!(matches!(SsrfFilter::validate_url("http://localhost/api"), Err(SsrfError::BlockedHost(_))));
        assert!(matches!(SsrfFilter::validate_url("http://127.0.0.1:8080"), Err(SsrfError::BlockedHost(_))));
        assert!(matches!(SsrfFilter::validate_url("http://127.1.2.3/data"), Err(SsrfError::BlockedHost(_))));
    }

    #[test]
    fn ssrf_blocks_private_rfc1918_ranges() {
        assert!(matches!(SsrfFilter::validate_url("http://10.0.0.1/admin"), Err(SsrfError::BlockedHost(_))));
        assert!(matches!(SsrfFilter::validate_url("http://172.16.1.1:3000"), Err(SsrfError::BlockedHost(_))));
        assert!(matches!(SsrfFilter::validate_url("http://192.168.1.1/router"), Err(SsrfError::BlockedHost(_))));
    }

    #[test]
    fn ssrf_blocks_cloud_metadata_ip() {
        assert!(matches!(
            SsrfFilter::validate_url("http://169.254.169.254/latest/meta-data"),
            Err(SsrfError::BlockedHost(_))
        ));
    }

    #[test]
    fn ssrf_permits_legitimate_public_urls() {
        assert_eq!(SsrfFilter::validate_url("https://api.aldivine.com/status"), Ok(()));
        assert_eq!(SsrfFilter::validate_url("https://github.com/resource.git"), Ok(()));
    }

    #[test]
    fn anticheat_detects_impossible_teleport() {
        let p1 = PositionSample { x: 0.0, y: 0.0, z: 72.0, timestamp_ms: 1000 };
        // Moved 500 meters in 1 second (500 m/s > max 50 m/s)
        let p2 = PositionSample { x: 500.0, y: 0.0, z: 72.0, timestamp_ms: 2000 };

        let res = AntiCheatDetector::verify_movement(p1, p2, 50.0);
        assert!(matches!(res, Err(AntiCheatViolation::SpeedHack { .. })));
    }

    #[test]
    fn anticheat_detects_out_of_world_bounds() {
        assert!(matches!(
            AntiCheatDetector::verify_world_bounds(50_000.0, 0.0, 72.0),
            Err(AntiCheatViolation::OutOfBounds { .. })
        ));
    }

    #[test]
    fn anticheat_permits_valid_movement_within_speed_limit() {
        let p1 = PositionSample { x: 0.0, y: 0.0, z: 72.0, timestamp_ms: 1000 };
        // Moved 10 meters in 1 second (10 m/s <= max 50 m/s)
        let p2 = PositionSample { x: 10.0, y: 0.0, z: 72.0, timestamp_ms: 2000 };

        assert_eq!(AntiCheatDetector::verify_movement(p1, p2, 50.0), Ok(()));
    }

    #[test]
    fn ddos_rate_limiter_throttles_bursts_exceeding_capacity() {
        let mut limiter = DdosRateLimiter::new(3, 1);
        let key = "192.168.1.100";

        // Consume 3 allowed tokens
        assert!(limiter.check_allow(key, 1000));
        assert!(limiter.check_allow(key, 1000));
        assert!(limiter.check_allow(key, 1000));

        // 4th token within same millisecond is rejected
        assert!(!limiter.check_allow(key, 1000));

        // After 2 seconds, tokens refill
        assert!(limiter.check_allow(key, 3000));
    }

    #[test]
    fn client_integrity_rejects_untrusted_hash() {
        let mut verifier = ClientIntegrityVerifier::new();
        verifier.register_allowed_build("build-1", "AABBCC112233");

        assert!(verifier.verify_client_hash("build-1", "aabbcc112233"));
        assert!(!verifier.verify_client_hash("build-1", "bad_hash_9999"));
        assert!(!verifier.verify_client_hash("unknown-build", "aabbcc112233"));
    }
}
