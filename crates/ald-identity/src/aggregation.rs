use crate::{Entitlement, SteamIdentity};
use ald_core::AldivinePlayerId;
use ald_device_identity::{Confidence, DeviceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RockstarIdentity {
    pub account_id: Option<String>,
    pub entitlement: Entitlement,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpicIdentity {
    pub account_id: Option<String>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub confidence: String,
    pub version: u8,
    pub first_seen: u64,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkIdentity {
    pub remote_ip: String,
}

/// Full aggregated identity for one player session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerIdentity {
    pub aldivine_id: AldivinePlayerId,
    pub rockstar: RockstarIdentity,
    pub steam: Option<SteamIdentity>,
    pub epic: EpicIdentity,
    pub device: Option<DeviceIdentity>,
    pub network: NetworkIdentity,
}

impl PlayerIdentity {
    pub fn new(aldivine_id: AldivinePlayerId) -> Self {
        PlayerIdentity {
            aldivine_id,
            rockstar: RockstarIdentity { account_id: None, entitlement: Entitlement::Unknown, verified: false },
            steam: None,
            epic: EpicIdentity { account_id: None, verified: false },
            device: None,
            network: NetworkIdentity { remote_ip: String::new() },
        }
    }

    pub fn attach_steam(&mut self, steam: SteamIdentity) {
        self.steam = Some(steam);
    }

    pub fn attach_device(&mut self, id: &DeviceId, confidence: Confidence, now: u64) {
        self.device = Some(DeviceIdentity {
            device_id: id.encoded(),
            confidence: confidence.as_str().to_string(),
            version: id.version,
            first_seen: now,
            last_seen: now,
        });
    }
}

/// Lightweight aggregation container passed through the pipeline.
#[derive(Debug, Clone, Default)]
pub struct IdentityAggregation {
    pub identities: Vec<PlayerIdentity>,
}

impl IdentityAggregation {
    pub fn new() -> Self {
        IdentityAggregation { identities: Vec::new() }
    }
    pub fn add(&mut self, p: PlayerIdentity) {
        self.identities.push(p);
    }
}
