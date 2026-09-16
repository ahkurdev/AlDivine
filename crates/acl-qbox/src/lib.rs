//! ACL adapter for Qbox-style resources.
//!
//! Qbox is a QBCore descendant, so this adapter delegates to the QBCore
//! implementation and only overrides the flavor and the capability list.
//! Keeping it a separate crate means a resource that declares
//! `compat = "qbox"` gets the qbox matrix in compatibility reports, without
//! the runtime having to special-case it.

use acl_qbcore::QbcoreAdapter;
use ald_compat::{Coverage, LegacyAdapter, LegacyFlavor, LegacyPlayer};

const QBOX_CAPS: &[&str] =
    &["GetPlayerByCitizenId", "PlayerData.money", "AddItem", "RemoveItem", "GetJob", "UpdatePlayerData"];

pub struct QboxAdapter<'svc>(pub QbcoreAdapter<'svc>);

impl LegacyAdapter for QboxAdapter<'_> {
    fn flavor(&self) -> LegacyFlavor {
        LegacyFlavor::Qbox
    }

    fn player(&self, source: u32) -> Option<LegacyPlayer> {
        self.0.player(source)
    }

    fn coverage(&self, capability: &str) -> Coverage {
        // Qbox dropped the QBCore.Functions.* namespace.
        match capability {
            "GetPlayerByCitizenId" | "PlayerData.money" | "GetJob" => Coverage::Supported,
            "AddItem" | "RemoveItem" | "UpdatePlayerData" => Coverage::Partial,
            _ => Coverage::NotSupported,
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        QBOX_CAPS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aldivine_framework::{EconomyService, InventoryService, JobService, PlayerService};
    use std::collections::HashMap;

    #[test]
    fn flavor_is_qbox_not_qbcore() {
        let players = PlayerService::new();
        let economy = EconomyService::new();
        let jobs = JobService::new();
        let inventory = InventoryService::new();
        let inner = QbcoreAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            citizens: HashMap::new(),
            sources: HashMap::new(),
        };
        let a = QboxAdapter(inner);
        assert_eq!(a.flavor(), LegacyFlavor::Qbox);
        assert_ne!(a.flavor(), LegacyFlavor::Qbcore);
    }

    #[test]
    fn qbox_capability_matrix_differs_from_qbcore() {
        let players = PlayerService::new();
        let economy = EconomyService::new();
        let jobs = JobService::new();
        let inventory = InventoryService::new();
        let inner = QbcoreAdapter {
            players: &players,
            economy: &economy,
            jobs: &jobs,
            inventory: &inventory,
            citizens: HashMap::new(),
            sources: HashMap::new(),
        };
        let a = QboxAdapter(inner);
        // Qbox namespace, no Functions prefix.
        assert_eq!(a.coverage("GetPlayerByCitizenId"), Coverage::Supported);
        // The QBCore-prefixed name is NOT a Qbox capability.
        assert_eq!(a.coverage("QBCore.Functions.GetPlayer"), Coverage::NotSupported);
        assert_eq!(a.coverage("AddItem"), Coverage::Partial);
    }
}
