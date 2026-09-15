//! ACL adapter for QBCore-style resources.
//!
//! Clean-room behavioral compatibility with the QBCore *player* API surface
//! (QBCore.Functions.GetPlayer, PlayerData money tables, item helpers).
//! Mutations are routed to the authoritative Aldivine services; nothing here
//! can create money or items on its own.

use std::collections::HashMap;

use ald_compat::{Coverage, LegacyAdapter, LegacyFlavor, LegacyPlayer};
use ald_core::AldivinePlayerId;
use aldivine_framework::{AccountKind, EconomyService, InventoryService, JobService, PlayerService};

pub struct QbcoreAdapter<'svc> {
    pub players: &'svc PlayerService,
    pub economy: &'svc EconomyService,
    pub jobs: &'svc JobService,
    pub inventory: &'svc InventoryService,
    /// QBCore citizenid -> Aldivine identity.
    pub citizens: HashMap<String, AldivinePlayerId>,
    /// Session source (numeric player index) -> Aldivine identity.
    pub sources: HashMap<u32, AldivinePlayerId>,
}

const CAPS: &[&str] = &[
    "QBCore.Functions.GetPlayer",
    "QBCore.Functions.GetPlayerByCitizenId",
    "PlayerData.money",
    "QBCore.Functions.AddItem",
    "QBCore.Functions.RemoveItem",
    "QBCore.Functions.GetJob",
    "QBCore.Player.UpdatePlayerData",
];

impl LegacyAdapter for QbcoreAdapter<'_> {
    fn flavor(&self) -> LegacyFlavor {
        LegacyFlavor::Qbcore
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
            "QBCore.Functions.GetPlayer"
            | "QBCore.Functions.GetPlayerByCitizenId"
            | "PlayerData.money"
            | "QBCore.Functions.GetJob" => Coverage::Supported,
            "QBCore.Functions.AddItem" | "QBCore.Functions.RemoveItem" | "QBCore.Player.UpdatePlayerData" => {
                Coverage::Partial
            }
            _ => Coverage::NotSupported,
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        CAPS
    }
}

impl QbcoreAdapter<'_> {
    /// QBCore keys players by citizenid; resolve it to the Aldivine identity.
    pub fn player_by_citizen(&self, citizenid: &str) -> Option<LegacyPlayer> {
        let id = *self.citizens.get(citizenid)?;
        let p = self.players.get(&id)?;
        let job = self.jobs.employment(&id);
        Some(LegacyPlayer {
            id,
            source: 0,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use aldivine_framework::{EconomyService, InventoryService, JobService, PlayerService};

    fn harness() -> (PlayerService, EconomyService, JobService, InventoryService, AldivinePlayerId) {
        let mut players = PlayerService::new();
        let mut economy = EconomyService::new();
        let id = AldivinePlayerId::new();
        players.join(id);
        economy.credit(id, AccountKind::Bank, 2000).unwrap();
        (players, economy, JobService::new(), InventoryService::new(), id)
    }

    #[test]
    fn flavor_is_qbcore() {
        let (players, economy, jobs, inventory, _) = harness();
        let a = QbcoreAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            citizens: HashMap::new(),
            sources: HashMap::new(),
        };
        assert_eq!(a.flavor(), LegacyFlavor::Qbcore);
    }

    #[test]
    fn coverage_matrix() {
        let (players, economy, jobs, inventory, _) = harness();
        let a = QbcoreAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            citizens: HashMap::new(),
            sources: HashMap::new(),
        };
        assert_eq!(a.coverage("QBCore.Functions.GetPlayer"), Coverage::Supported);
        assert_eq!(a.coverage("QBCore.Functions.AddItem"), Coverage::Partial);
        assert_eq!(a.coverage("QBCore.Functions.CreateBill"), Coverage::NotSupported);
    }

    #[test]
    fn player_by_citizenid() {
        let (players, economy, jobs, inventory, id) = harness();
        let mut citizens = HashMap::new();
        citizens.insert("ABC12345".into(), id);
        let a = QbcoreAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            citizens,
            sources: HashMap::new(),
        };
        let p = a.player_by_citizen("ABC12345").unwrap();
        assert_eq!(p.money, 2000);
        assert!(a.player_by_citizen("MISSING").is_none());
    }
}
