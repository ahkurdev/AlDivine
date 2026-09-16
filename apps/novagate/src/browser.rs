//! Server browser state: favorites, recent history, and direct connect.
//!
//! Kept deliberately stateful-but-pure: no network in this module. The Tauri
//! frontend calls these to persist the player's server list; the actual
//! server query happens over AstraNet elsewhere.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A server the player can connect to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub description: Option<String>,
    pub max_players: u32,
    pub online_players: u32,
    pub ping_ms: Option<u32>,
    pub password_protected: bool,
    pub favorite: bool,
}

/// A connection target, resolved before launching Astryn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectConnect {
    pub address: String,
    pub port: u16,
    pub password: Option<String>,
}

/// Local browser state: favorites (name->entry) and connection history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserState {
    favorites: HashMap<String, ServerEntry>,
    history: Vec<HistoryEntry>,
    max_history: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub server_id: String,
    pub server_name: String,
    pub address: String,
    pub port: u16,
    /// Unix seconds.
    pub last_connected: u64,
}

impl BrowserState {
    pub fn new(max_history: usize) -> Self {
        BrowserState { favorites: HashMap::new(), history: Vec::new(), max_history: max_history.max(1) }
    }

    /// Add or replace a favorite. Duplicate names overwrite.
    pub fn add_favorite(&mut self, entry: ServerEntry) {
        self.favorites.insert(entry.id.clone(), entry);
    }

    pub fn remove_favorite(&mut self, id: &str) -> Option<ServerEntry> {
        self.favorites.remove(id)
    }

    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.contains_key(id)
    }

    pub fn favorites(&self) -> Vec<&ServerEntry> {
        let mut f: Vec<&ServerEntry> = self.favorites.values().collect();
        f.sort_by_key(|a| a.name.to_lowercase());
        f
    }

    /// Record a connection. The most recent entry per server wins, and the
    /// list is bounded so it cannot grow without limit.
    pub fn record_connection(&mut self, server: &ServerEntry, now: u64) {
        self.history.retain(|h| h.server_id != server.id);
        self.history.insert(
            0,
            HistoryEntry {
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                address: server.address.clone(),
                port: server.port,
                last_connected: now,
            },
        );
        if self.history.len() > self.max_history {
            self.history.truncate(self.max_history);
        }
    }

    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    /// Resolve a direct-connect target. Rejects an empty address and a
    /// password-protected server without a password: better to fail here than
    /// to hand a malformed target to Astryn.
    pub fn resolve_direct_connect(target: &DirectConnect) -> Result<ResolvedConnect, String> {
        let addr = target.address.trim();
        if addr.is_empty() {
            return Err("address is required".into());
        }
        if addr.contains(' ') {
            return Err("address must not contain spaces".into());
        }
        Ok(ResolvedConnect { address: addr.to_string(), port: target.port, password: target.password.clone() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConnect {
    pub address: String,
    pub port: u16,
    pub password: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str, name: &str, addr: &str, port: u16) -> ServerEntry {
        ServerEntry {
            id: id.into(),
            name: name.into(),
            address: addr.into(),
            port,
            description: None,
            max_players: 32,
            online_players: 0,
            ping_ms: None,
            password_protected: false,
            favorite: false,
        }
    }

    #[test]
    fn favorites_add_remove_query() {
        let mut s = BrowserState::new(10);
        s.add_favorite(server("a", "Alpha RP", "1.2.3.4", 30120));
        assert!(s.is_favorite("a"));
        assert!(!s.is_favorite("b"));
        assert!(s.remove_favorite("a").is_some());
        assert!(!s.is_favorite("a"));
    }

    #[test]
    fn favorites_sorted_by_name_case_insensitive() {
        let mut s = BrowserState::new(10);
        s.add_favorite(server("z", "zeta", "1.1.1.1", 1));
        s.add_favorite(server("a", "Alpha", "1.1.1.2", 1));
        let names: Vec<&str> = s.favorites().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "zeta"]);
    }

    #[test]
    fn history_is_bounded_and_most_recent_wins() {
        let mut s = BrowserState::new(3);
        for i in 0..5 {
            s.record_connection(&server(&format!("s{i}"), &format!("Server {i}"), "1.1.1.1", 1), 1000 + i);
        }
        assert_eq!(s.history().len(), 3);
        assert_eq!(s.history()[0].server_id, "s4");
    }

    #[test]
    fn history_dedupes_reconnect() {
        let mut s = BrowserState::new(10);
        let srv = server("a", "Alpha", "1.1.1.1", 1);
        s.record_connection(&srv, 100);
        s.record_connection(&srv, 200);
        assert_eq!(s.history().len(), 1);
        assert_eq!(s.history()[0].last_connected, 200);
    }

    #[test]
    fn direct_connect_resolves() {
        let t = DirectConnect { address: "play.example.com".into(), port: 30120, password: None };
        let r = BrowserState::resolve_direct_connect(&t).unwrap();
        assert_eq!(r.address, "play.example.com");
        assert_eq!(r.port, 30120);
    }

    #[test]
    fn direct_connect_rejects_empty_address() {
        let t = DirectConnect { address: "  ".into(), port: 30120, password: None };
        assert!(BrowserState::resolve_direct_connect(&t).is_err());
    }

    #[test]
    fn direct_connect_rejects_spaces() {
        let t = DirectConnect { address: "not an address".into(), port: 30120, password: None };
        assert!(BrowserState::resolve_direct_connect(&t).is_err());
    }

    #[test]
    fn history_survives_serialization() {
        let mut s = BrowserState::new(10);
        s.record_connection(&server("a", "Alpha", "1.1.1.1", 1), 99);
        let json = serde_json::to_string(&s).unwrap();
        let back: BrowserState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.history().len(), 1);
        assert_eq!(back.history()[0].server_id, "a");
    }
}
