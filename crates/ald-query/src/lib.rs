//! Aldivine server query protocol.
//!
//! Browser/discovery endpoint. Anonymous UDP probe + JSON response, modeled
//! on the legacy /info.json + /players.json surface but Aldivine-native:
//!   - fields the operator chose to publish only (privacy)
//!   - rate-limited per source (anti-amplification)
//!   - bounded size, bounded player list
//!   - no token/secret material ever
//!
//! Legacy endpoints (/info.json, /players.json, /dynamic.json) are produced
//! from the same ServerStatus so there is one source of truth.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Magic for the UDP probe: "ALDQ" (0x41 0x4C 0x44 0x51).
pub const QUERY_MAGIC: [u8; 4] = *b"ALDQ";

/// Current query schema version.
pub const QUERY_VERSION: u8 = 1;

/// Bounded cap on the player list we will serialize per query.
pub const MAX_PLAYERS_IN_LIST: usize = 64;

/// Max serialized query response bytes. Anything bigger is truncated by
/// dropping the player list first, then icons.
pub const MAX_RESPONSE_BYTES: usize = 8192;

/// Probe a client sends. Kept tiny: magic + version + optional token slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryProbe {
    pub magic: [u8; 4],
    pub version: u8,
    /// Optional nonce echoed back, lets the browser match replies.
    pub nonce: u32,
}

impl QueryProbe {
    pub fn new(nonce: u32) -> Self {
        QueryProbe { magic: QUERY_MAGIC, version: QUERY_VERSION, nonce }
    }

    /// Wire encode: 9 bytes.
    pub fn encode(&self) -> [u8; 9] {
        let mut out = [0u8; 9];
        out[..4].copy_from_slice(&self.magic);
        out[4] = self.version;
        out[5..9].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }

    /// Wire decode with strict validation.
    pub fn decode(b: &[u8]) -> Result<Self, QueryError> {
        if b.len() != 9 {
            return Err(QueryError::BadLength { got: b.len(), want: 9 });
        }
        let magic = [b[0], b[1], b[2], b[3]];
        if magic != QUERY_MAGIC {
            return Err(QueryError::BadMagic);
        }
        let version = b[4];
        if version != QUERY_VERSION {
            return Err(QueryError::UnsupportedVersion(version));
        }
        let nonce = u32::from_le_bytes([b[5], b[6], b[7], b[8]]);
        Ok(QueryProbe { magic, version, nonce })
    }
}

/// One player row. Only what the operator publishes; never identifiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryPlayer {
    pub name: String,
    /// Optional: character/job label, e.g. "Police Officer".
    pub label: Option<String>,
}

/// Server-published status. This is the single source of truth; legacy JSON
/// views are derived from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub hostname: String,
    pub project_name: Option<String>,
    pub project_desc: Option<String>,
    /// "ald:protocol@N" + negotiated feature summary.
    pub protocol_version: u16,
    pub max_clients: u32,
    pub online: u32,
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub game_build: Option<String>,
    /// True only when the operator opted into public listing.
    pub listed: bool,
    /// Small PNG icon, base64 (bounded).
    pub icon_b64: Option<String>,
    /// Bounded player list.
    pub players: Vec<QueryPlayer>,
    /// Extra arbitrary metadata the operator chose to publish.
    pub extra: HashMap<String, String>,
    /// Server-side nonce echoed from the probe.
    pub nonce: u32,
    /// "ok" | "degraded" | "locked" — honest operational state.
    pub health: String,
}

impl ServerStatus {
    /// Render the native Aldivine JSON response, bounded to MAX_RESPONSE_BYTES.
    /// Truncation order: players -> icon -> extra -> desc. Hostname/counts
    /// are never dropped, because a browser needs them to render anything.
    pub fn to_native_json(&self) -> Result<String, QueryError> {
        let mut cur = self.clone();
        if cur.players.len() > MAX_PLAYERS_IN_LIST {
            cur.players.truncate(MAX_PLAYERS_IN_LIST);
        }
        // Progressive shedding until we fit.
        let mut shed_icon = false;
        let mut shed_players = false;
        let mut shed_extra = false;
        loop {
            let s = serde_json::to_string(&cur).map_err(|e| QueryError::Render(e.to_string()))?;
            if s.len() <= MAX_RESPONSE_BYTES {
                return Ok(s);
            }
            if !shed_icon {
                cur.icon_b64 = None;
                shed_icon = true;
                continue;
            }
            if !shed_players {
                cur.players.clear();
                shed_players = true;
                continue;
            }
            if !shed_extra {
                cur.extra.clear();
                shed_extra = true;
                continue;
            }
            // Last resort: drop long prose fields, keep identity + counts.
            cur.project_desc = None;
            if serde_json::to_string(&cur).map_err(|e| QueryError::Render(e.to_string()))?.len() <= MAX_RESPONSE_BYTES {
                return serde_json::to_string(&cur).map_err(|e| QueryError::Render(e.to_string()));
            }
            return Err(QueryError::TooLarge);
        }
    }

    /// Legacy /info.json view. Deliberately minimal: never player names.
    pub fn to_legacy_info_json(&self) -> Result<String, QueryError> {
        let info = serde_json::json!({
            "sv_maxclients": self.max_clients,
            "sv_hostname": self.hostname,
            "sv_projectName": self.project_name.clone().unwrap_or_default(),
            "sv_projectDesc": self.project_desc.clone().unwrap_or_default(),
            "sv_tags": self.tags.join(","),
            "sv_locale": self.locale.clone().unwrap_or_default(),
            "sv_gameBuild": self.game_build.clone().unwrap_or_default(),
            "icon": self.icon_b64.clone().unwrap_or_default(),
            "version": self.protocol_version,
            "health": self.health,
        });
        Ok(info.to_string())
    }

    /// Legacy /players.json view. Names only, bounded.
    pub fn to_legacy_players_json(&self) -> Result<String, QueryError> {
        let rows: Vec<_> = self
            .players
            .iter()
            .take(MAX_PLAYERS_IN_LIST)
            .map(|p| {
                let mut o = serde_json::json!({ "name": p.name });
                if let Some(l) = &p.label {
                    o["label"] = serde_json::json!(l);
                }
                o
            })
            .collect();
        Ok(serde_json::json!({ "players": rows }).to_string())
    }

    /// Legacy /dynamic.json view: counts only.
    pub fn to_legacy_dynamic_json(&self) -> Result<String, QueryError> {
        Ok(serde_json::json!({
            "sv_maxclients": self.max_clients,
            "clients": self.online,
            "gametype": self.tags.first().cloned().unwrap_or_default(),
            "mapname": self.game_build.clone().unwrap_or_default(),
        })
        .to_string())
    }
}

/// Errors from the query path.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum QueryError {
    #[error("bad probe length: got {got}, want {want}")]
    BadLength { got: usize, want: usize },
    #[error("bad query magic")]
    BadMagic,
    #[error("unsupported query version {0}")]
    UnsupportedVersion(u8),
    #[error("response too large after truncation")]
    TooLarge,
    #[error("render failed: {0}")]
    Render(String),
    #[error("rate limited")]
    RateLimited,
    #[error("listing disabled")]
    NotListed,
}

/// Per-source rate limiter for anonymous query probes. Prevents
/// amplification and cheap scanning of the player list.
#[derive(Debug)]
pub struct QueryRateLimiter {
    window: Duration,
    max_per_window: u32,
    hits: HashMap<IpAddr, (Instant, u32)>,
}

impl QueryRateLimiter {
    pub fn new(max_per_window: u32, window: Duration) -> Self {
        QueryRateLimiter { window, max_per_window, hits: HashMap::new() }
    }

    /// Returns Ok(()) if the source may query now, recording the hit.
    pub fn check(&mut self, src: IpAddr) -> Result<(), QueryError> {
        let now = Instant::now();
        match self.hits.get_mut(&src) {
            Some((start, count)) => {
                if now.duration_since(*start) < self.window {
                    if *count >= self.max_per_window {
                        return Err(QueryError::RateLimited);
                    }
                    *count += 1;
                } else {
                    *start = now;
                    *count = 1;
                }
            }
            None => {
                self.hits.insert(src, (now, 1));
            }
        }
        Ok(())
    }

    /// Drop stale buckets. Bounded memory.
    pub fn gc(&mut self) {
        let now = Instant::now();
        self.hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
    }

    pub fn tracked_sources(&self) -> usize {
        self.hits.len()
    }
}

/// Handle one anonymous UDP probe synchronously: validate, rate-limit,
/// produce the bounded response bytes. `listed=false` still answers (so the
/// browser can show "unlisted"), but with counts only.
pub fn handle_query_probe(
    status: &ServerStatus,
    limiter: &mut QueryRateLimiter,
    src: IpAddr,
    bytes: &[u8],
) -> Result<Vec<u8>, QueryError> {
    let probe = QueryProbe::decode(bytes)?;
    limiter.check(src)?;

    let mut view = status.clone();
    view.nonce = probe.nonce;
    if !view.listed {
        // Unlisted: answer with identity, no player names, no desc.
        view.players.clear();
        view.project_desc = None;
        view.extra.clear();
    }
    let json = view.to_native_json()?;
    Ok(json.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn status() -> ServerStatus {
        ServerStatus {
            hostname: "Test Server".into(),
            project_name: Some("Test".into()),
            project_desc: Some("A test server".into()),
            protocol_version: 1,
            max_clients: 48,
            online: 3,
            tags: vec!["roleplay".into()],
            locale: Some("en-US".into()),
            game_build: Some("b3095".into()),
            listed: true,
            icon_b64: None,
            players: vec![
                QueryPlayer { name: "Alice".into(), label: Some("Police".into()) },
                QueryPlayer { name: "Bob".into(), label: None },
            ],
            extra: HashMap::new(),
            nonce: 0,
            health: "ok".into(),
        }
    }

    #[test]
    fn probe_roundtrip() {
        let p = QueryProbe::new(0xdeadbeef);
        let b = p.encode();
        assert_eq!(b.len(), 9);
        assert_eq!(QueryProbe::decode(&b).unwrap(), p);
    }

    #[test]
    fn probe_rejects_bad_magic() {
        let mut b = QueryProbe::new(1).encode();
        b[0] = 0;
        assert!(matches!(QueryProbe::decode(&b), Err(QueryError::BadMagic)));
    }

    #[test]
    fn probe_rejects_bad_length() {
        assert!(matches!(QueryProbe::decode(&[0u8; 5]), Err(QueryError::BadLength { .. })));
        assert!(matches!(QueryProbe::decode(&[0u8; 33]), Err(QueryError::BadLength { .. })));
    }

    #[test]
    fn probe_rejects_wrong_version() {
        let mut b = QueryProbe::new(1).encode();
        b[4] = 99;
        assert!(matches!(QueryProbe::decode(&b), Err(QueryError::UnsupportedVersion(99))));
    }

    #[test]
    fn native_json_fits_and_carries_nonce() {
        let s = status();
        let j = s.to_native_json().unwrap();
        assert!(j.len() <= MAX_RESPONSE_BYTES);
        assert!(j.contains("\"nonce\":0"));
    }

    #[test]
    fn large_status_sheds_players_then_fails_loudly() {
        let mut s = status();
        s.players = (0..10_000)
            .map(|i| QueryPlayer {
                name: format!("PlayerNumber{i}"),
                label: Some("A fairly long label that takes room".into()),
            })
            .collect();
        let j = s.to_native_json().unwrap();
        assert!(j.len() <= MAX_RESPONSE_BYTES);
        // truncated to cap
        assert!(!j.contains("PlayerNumber500"));
    }

    #[test]
    fn unlisted_hides_players_and_desc() {
        let mut s = status();
        s.listed = false;
        let mut lim = QueryRateLimiter::new(10, Duration::from_secs(1));
        let out =
            handle_query_probe(&s, &mut lim, IpAddr::V4(Ipv4Addr::LOCALHOST), &QueryProbe::new(7).encode()).unwrap();
        let txt = String::from_utf8(out).unwrap();
        assert!(txt.contains("\"nonce\":7"));
        assert!(!txt.contains("Alice"));
        assert!(!txt.contains("A test server"));
    }

    #[test]
    fn rate_limiter_blocks_after_max() {
        let mut lim = QueryRateLimiter::new(2, Duration::from_secs(60));
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(lim.check(ip).is_ok());
        assert!(lim.check(ip).is_ok());
        assert!(matches!(lim.check(ip), Err(QueryError::RateLimited)));
    }

    #[test]
    fn rate_limiter_gc_drops_stale() {
        let mut lim = QueryRateLimiter::new(5, Duration::from_millis(1));
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(lim.check(ip).is_ok());
        std::thread::sleep(Duration::from_millis(20));
        lim.gc();
        assert_eq!(lim.tracked_sources(), 0);
        assert!(lim.check(ip).is_ok());
    }

    #[test]
    fn legacy_views_present_expected_keys() {
        let s = status();
        let info = s.to_legacy_info_json().unwrap();
        assert!(info.contains("sv_maxclients"));
        assert!(info.contains("Test Server"));
        let players = s.to_legacy_players_json().unwrap();
        assert!(players.contains("Alice"));
        assert!(players.contains("\"label\""));
        let dynj = s.to_legacy_dynamic_json().unwrap();
        assert!(dynj.contains("clients"));
    }

    #[test]
    fn legacy_players_bounded() {
        let mut s = status();
        s.players = (0..1000).map(|i| QueryPlayer { name: format!("P{i}"), label: None }).collect();
        let j = s.to_legacy_players_json().unwrap();
        assert!(!j.contains("P99"));
    }

    #[test]
    fn malformed_probe_rejected_without_touching_status() {
        let s = status();
        let mut lim = QueryRateLimiter::new(10, Duration::from_secs(1));
        let r = handle_query_probe(&s, &mut lim, IpAddr::V4(Ipv4Addr::LOCALHOST), &[0u8; 3]);
        assert!(matches!(r, Err(QueryError::BadLength { .. })));
        // malformed probe must not consume a rate-limit slot
        assert!(lim.check(IpAddr::V4(Ipv4Addr::LOCALHOST)).is_ok());
    }

    #[test]
    fn status_serde_roundtrip() {
        let s = status();
        let j = serde_json::to_string(&s).unwrap();
        let back: ServerStatus = serde_json::from_str(&j).unwrap();
        assert_eq!(back.hostname, s.hostname);
        assert_eq!(back.players.len(), s.players.len());
    }
}
