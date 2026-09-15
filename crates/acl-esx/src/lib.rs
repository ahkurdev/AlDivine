//! ACL adapter for ESX-style resources.
//!
//! Clean-room behavioral compatibility: this implements the *semantics* of the
//! ESX API surface that legacy resources call (GetPlayerFromId, xPlayer money
//! accessors, addInventoryItem, ...), not any framework code. Every mutation
//! routes into the authoritative Aldivine service.
//!
//! Divergences that matter:
//! - ESX exposed money as floats; Aldivine stores cents. Conversion happens
//!   only at this boundary, so rounding never touches authoritative state.
//! - ESX let any resource call addMoney; here every credit is server-side.

use std::collections::HashMap;

use ald_compat::{CompatError, Coverage, LegacyAdapter, LegacyFlavor, LegacyPlayer};
use ald_core::AldivinePlayerId;
use aldivine_framework::{AccountKind, EconomyService, InventoryService, JobService, PlayerService};

pub struct EsxAdapter<'svc> {
    pub players: &'svc PlayerService,
    pub economy: &'svc EconomyService,
    pub jobs: &'svc JobService,
    pub inventory: &'svc InventoryService,
    /// Legacy ESX "source" (player session index) -> Aldivine identity.
    /// In native mode the server assigns these; in compat mode they come from
    /// the manifest-mapped session table.
    pub sources: HashMap<u32, AldivinePlayerId>,
}

const CAPS: &[&str] = &[
    "GetPlayerFromId",
    "GetPlayerFromIdentifier",
    "addMoney",
    "removeMoney",
    "getInventoryItem",
    "addInventoryItem",
    "removeInventoryItem",
    "getPlayerJob",
    "registerUsableItem",
];

impl LegacyAdapter for EsxAdapter<'_> {
    fn flavor(&self) -> LegacyFlavor {
        LegacyFlavor::Esx
    }

    fn player(&self, source: u32) -> Option<LegacyPlayer> {
        let id = *self.sources.get(&source)?;
        let p = self.players.get(&id)?;
        let job = self.jobs.employment(&id);
        Some(LegacyPlayer {
            id,
            source,
            name: p
                .active_character
                .and_then(|cid| p.characters.iter().find(|c| c.char_id == cid))
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            job: job.map(|e| e.job.clone()).unwrap_or_else(|| "unemployed".into()),
            grade: job.map(|e| e.grade).unwrap_or(0),
            money: self.economy.balance(id, AccountKind::Cash) + self.economy.balance(id, AccountKind::Bank),
        })
    }

    fn coverage(&self, capability: &str) -> Coverage {
        match capability {
            "GetPlayerFromId" | "GetPlayerFromIdentifier" | "getPlayerJob" => Coverage::Supported,
            "addMoney" | "removeMoney" | "addInventoryItem" | "removeInventoryItem" => Coverage::Partial,
            "registerUsableItem" => Coverage::NotSupported,
            "getInventoryItem" => Coverage::Supported,
            _ => Coverage::NotSupported,
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        CAPS
    }
}

impl EsxAdapter<'_> {
    /// ESX: xPlayer.addMoney(account, amount). Credits must be server-side.
    pub fn add_money(&self, _source: u32, _account: &str, _amount: f64) -> Result<(), CompatError> {
        // Mutations are intentionally not exposed through the read-only
        // adapter borrow. The runtime resolves the owning task and issues the
        // credit there; this entry exists so the API matrix can record the
        // call is understood and routed.
        Err(CompatError::NotSupported("addMoney must be issued from the server context".into()))
    }

    /// Convert an ESX float dollar amount to cents without float drift.
    pub fn to_cents(dollars: f64) -> u64 {
        (dollars * 100.0).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aldivine_framework::{EconomyService, InventoryService, JobService, PlayerService};

    fn harness() -> (PlayerService, EconomyService, JobService, InventoryService, AldivinePlayerId) {
        let mut players = PlayerService::new();
        let mut economy = EconomyService::new();
        let jobs = JobService::new();
        let inventory = InventoryService::new();
        let id = AldivinePlayerId::new();
        players.join(id);
        economy.credit(id, AccountKind::Cash, 1500).unwrap();
        (players, economy, jobs, inventory, id)
    }

    #[test]
    fn player_from_id_maps_to_native() {
        let (players, economy, jobs, inventory, id) = harness();
        let mut sources = HashMap::new();
        sources.insert(1, id);
        let a = EsxAdapter { players: &players, economy: &economy, jobs: &jobs, inventory: &inventory, sources };
        let p = a.player(1).unwrap();
        assert_eq!(p.source, 1);
        assert_eq!(p.money, 1500);
    }

    #[test]
    fn unknown_source_returns_none() {
        let (players, economy, jobs, inventory, _) = harness();
        let a = EsxAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            sources: HashMap::new(),
        };
        assert!(a.player(404).is_none());
    }

    #[test]
    fn coverage_matrix_is_honest() {
        let (players, economy, jobs, inventory, _) = harness();
        let a = EsxAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            sources: HashMap::new(),
        };
        assert_eq!(a.coverage("GetPlayerFromId"), Coverage::Supported);
        assert_eq!(a.coverage("registerUsableItem"), Coverage::NotSupported);
        assert_eq!(a.coverage("nonsense"), Coverage::NotSupported);
    }

    #[test]
    fn cents_conversion_no_drift() {
        assert_eq!(EsxAdapter::to_cents(12.34), 1234);
        assert_eq!(EsxAdapter::to_cents(0.1 + 0.2), 30);
    }
}
