//! AstraNet — Aldivine networking subsystem.
//!
//! Hybrid transport: UDP datagrams for latency-critical state, optional QUIC
//! streams/datagrams for reliable bulk transfer. Logical channels separate
//! control, auth, events, entity state, resource transfer, admin, heartbeat
//! and voice metadata.
//!
//! Security: every ingress packet is version-checked, size-checked, replay-
//! protected and firewall-evaluated before any gameplay code runs.

pub mod firewall;
pub mod ratelimit;
pub mod reliability;
pub mod session;
pub mod transport;

pub use firewall::{Direction, EventFirewall, EventRule, FirewallVerdict};
pub use ratelimit::RateLimiter;
pub use reliability::{ReliabilityError, ReliabilityState};
pub use session::{Session, SessionManager};
pub use transport::{fragment, UdpTransport, MTU};

// IP normalization and trusted-proxy handling (existing module body below).
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ald_core::AldError;
use serde::{Deserialize, Serialize};

/// Normalize an IP string. Handles IPv4, IPv6, and IPv4-mapped IPv6
/// (::ffff:1.2.3.4 -> 1.2.3.4). Invalid input is an error.
pub fn normalize_ip(s: &str) -> Result<IpAddr, AldError> {
    let s = s.trim().trim_matches('"');
    let s = s.split(',').next().unwrap_or(s).trim(); // first hop of XFF if present
    let ip: IpAddr = s.parse().map_err(|_| AldError::InvalidArgument(format!("invalid ip: {s}")))?;
    Ok(canonicalize(ip))
}

fn canonicalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                IpAddr::V4(v4)
            } else {
                IpAddr::V6(v6)
            }
        }
        other => other,
    }
}

/// A CIDR range used for trusted-proxy configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cidr {
    pub base: IpAddr,
    pub prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Self, AldError> {
        let (addr, prefix) = s.split_once('/').ok_or_else(|| AldError::InvalidArgument("missing /prefix".into()))?;
        let base: IpAddr =
            addr.trim().parse().map_err(|_| AldError::InvalidArgument(format!("bad cidr addr: {addr}")))?;
        let prefix: u8 = prefix.trim().parse().map_err(|_| AldError::InvalidArgument("bad prefix".into()))?;
        let max = if base.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(AldError::InvalidArgument("prefix out of range".into()));
        }
        Ok(Cidr { base, prefix })
    }

    /// True if `ip` is within this CIDR.
    pub fn contains(&self, ip: IpAddr) -> bool {
        if self.base.is_ipv4() != ip.is_ipv4() {
            return false;
        }
        match (self.base, ip) {
            (IpAddr::V4(b), IpAddr::V4(a)) => in_range_v4(a, b, self.prefix),
            (IpAddr::V6(b), IpAddr::V6(a)) => in_range_v6(a, b, self.prefix),
            _ => false,
        }
    }
}

fn in_range_v4(addr: Ipv4Addr, base: Ipv4Addr, prefix: u8) -> bool {
    let a = u32::from(addr);
    let b = u32::from(base);
    if prefix == 0 {
        return true;
    }
    let mask = if prefix >= 32 { u32::MAX } else { !(u32::MAX >> prefix) };
    (a & mask) == (b & mask)
}

fn in_range_v6(addr: Ipv6Addr, base: Ipv6Addr, prefix: u8) -> bool {
    let a = addr.octets();
    let b = base.octets();
    let mut bits = prefix as i32;
    for i in 0..16 {
        if bits <= 0 {
            break;
        }
        let take = bits.min(8) as u8;
        let mask = if take == 8 { 0xFFu8 } else { 0xFFu8 << (8 - take) };
        if (a[i] & mask) != (b[i] & mask) {
            return false;
        }
        bits -= 8;
    }
    true
}

/// Resolve the authoritative client IP given the observed socket IP and an
/// optional forwarded value + trusted proxy list. Untrusted forwarded values
/// are ignored.
pub fn resolve_client_ip(observed: IpAddr, forwarded: Option<&str>, trusted: &[Cidr]) -> IpAddr {
    let trusted_observed = trusted.iter().any(|c| c.contains(observed));
    if trusted_observed {
        if let Some(fwd) = forwarded {
            if let Ok(ip) = normalize_ip(fwd) {
                return ip;
            }
        }
    }
    observed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ipv4_mapped() {
        let ip = normalize_ip("::ffff:192.0.2.1").unwrap();
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)));
    }

    #[test]
    fn normalize_xff_first_hop() {
        let ip = normalize_ip("203.0.113.5, 70.41.3.18, 150.172.238.178").unwrap();
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)));
    }

    #[test]
    fn cidr_match_v4() {
        let c = Cidr::parse("10.0.0.0/24").unwrap();
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 42))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1))));
    }

    #[test]
    fn cidr_match_v6() {
        let c = Cidr::parse("2001:db8::/32").unwrap();
        assert!(c.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))));
        assert!(!c.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn trusted_proxy_uses_forwarded() {
        let obs = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10));
        let c = Cidr::parse("10.0.0.0/24").unwrap();
        let got = resolve_client_ip(obs, Some("203.0.113.9"), &[c]);
        assert_eq!(got, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)));
    }

    #[test]
    fn untrusted_proxy_ignored() {
        let obs = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        let got = resolve_client_ip(obs, Some("203.0.113.9"), &[]);
        assert_eq!(got, obs);
    }
}
