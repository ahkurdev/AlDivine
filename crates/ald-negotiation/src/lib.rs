//! Connection-time feature negotiation + protocol deprecation UX.
//!
//! Both sides declare the protocol/feature set they support. The *server* is
//! authoritative: it decides the negotiated set. Outcomes are exact, typed,
//! and carry the upgrade action a human needs — never a bare "rejected".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use ald_protocol::PROTOCOL_VERSION;

/// A single feature that can be negotiated at connect time.
/// Versioned independently of the wire protocol.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FeatureId {
    pub namespace: String,
    pub name: String,
    pub version: u32,
}

impl FeatureId {
    pub fn new(namespace: &str, name: &str, version: u32) -> Self {
        FeatureId { namespace: namespace.to_string(), name: name.to_string(), version }
    }

    /// Wire key: "namespace:name@version".
    pub fn key(&self) -> String {
        format!("{}:{}@{}", self.namespace, self.name, self.version)
    }

    /// Same feature identity, ignoring version.
    pub fn same_feature(&self, other: &Self) -> bool {
        self.namespace == other.namespace && self.name == other.name
    }
}

impl std::fmt::Display for FeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}

/// One side of the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Implementation name ("astryn", "ald-server", "astrasim").
    pub impl_name: String,
    pub impl_version: String,
    /// Highest wire protocol version the peer speaks.
    pub protocol_max: u16,
    pub protocol_min: u16,
    /// Features this peer offers.
    pub features: Vec<FeatureId>,
    /// Engine-reported capabilities (e.g. "dui", "nui-wasm", "voice-opus").
    pub capabilities: Vec<String>,
}

impl Hello {
    pub fn new(impl_name: &str, impl_version: &str) -> Self {
        Hello {
            impl_name: impl_name.to_string(),
            impl_version: impl_version.to_string(),
            protocol_max: PROTOCOL_VERSION,
            protocol_min: 1,
            features: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn with_feature(mut self, f: FeatureId) -> Self {
        self.features.push(f);
        self
    }

    pub fn with_capability(mut self, c: &str) -> Self {
        self.capabilities.push(c.to_string());
        self
    }

    pub fn has_capability(&self, c: &str) -> bool {
        self.capabilities.iter().any(|x| x == c)
    }
}

/// Outcome of negotiating one feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureOutcome {
    /// Both sides offer it; server picked this version.
    Selected(FeatureId),
    /// Client wants it but server never heard of it.
    ClientOnlyUnsupported(FeatureId),
    /// Server has it, client too old to use it.
    RequiresClientUpdate(FeatureId),
    /// Server has it, client refuses / too new for this server.
    RequiresServerUpdate(FeatureId),
    /// Explicitly turned off by server policy.
    DisabledByPolicy(FeatureId),
}

/// Protocol-version negotiation result. Carries the structured upgrade UX.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolNegotiation {
    /// Wire version both sides agreed on.
    Agreed { version: u16 },
    /// Client older than the server's minimum.
    ClientTooOld { client_version: u16, server_min: u16, server_max: u16, action: UpgradeAction },
    /// Client newer than anything the server speaks.
    ServerTooOld { client_version: u16, server_max: u16, action: UpgradeAction },
    /// Protocol family mismatch (e.g. wrong magic/channel layout).
    Incompatible { reason: String, action: UpgradeAction },
}

/// What a human/UI must do to resolve a version gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpgradeAction {
    /// Download a newer client (NovaGate update).
    UpdateClient { channel: String, min_version: String },
    /// Server operator must update the server runtime.
    UpdateServer { channel: String, min_version: String },
    /// Mixed client/server suite is unsupported together.
    UnsupportedCombination,
    /// Nothing to do — informative only.
    None,
}

/// Full result of a handshake, server-authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiatedSet {
    pub protocol: ProtocolNegotiation,
    /// Per-feature outcomes, keyed by feature key.
    pub features: BTreeMap<String, FeatureOutcome>,
    /// Capabilities both sides reported (intersection).
    pub shared_capabilities: Vec<String>,
    /// True when the connection may proceed.
    pub allowed: bool,
    /// Human-readable summary for the F8 console / Aegis.
    pub summary: String,
}

/// Server-side policy that governs negotiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiationPolicy {
    pub server_protocol_min: u16,
    pub server_protocol_max: u16,
    /// Features the server offers.
    pub server_features: Vec<FeatureId>,
    /// Free-form capabilities the server can serve (e.g. "voice-opus",
    /// "nui-wasm"). Independent of feature versions.
    pub server_capabilities: Vec<String>,
    /// Features administratively disabled.
    pub disabled: Vec<String>,
    /// Reject (rather than warn) when a required client feature is missing.
    pub strict_required_features: Vec<FeatureId>,
    /// Upgrade-channel metadata surfaced in UpgradeAction.
    pub client_channel: String,
    pub client_min_version: String,
    pub server_channel: String,
    pub server_min_version: String,
}

impl Default for NegotiationPolicy {
    fn default() -> Self {
        NegotiationPolicy {
            server_protocol_min: 1,
            server_protocol_max: PROTOCOL_VERSION,
            server_features: Vec::new(),
            server_capabilities: Vec::new(),
            disabled: Vec::new(),
            strict_required_features: Vec::new(),
            client_channel: "stable".to_string(),
            client_min_version: "1.0.0".to_string(),
            server_channel: "stable".to_string(),
            server_min_version: "1.0.0".to_string(),
        }
    }
}

/// Errors that terminate the handshake. These map 1:1 to the structured
/// connection codes in the identity pipeline (ALD-* codes).
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum NegotiationError {
    #[error("client protocol {client_version} too old; server requires >= {server_min}")]
    ClientTooOld { client_version: u16, server_min: u16, server_max: u16 },
    #[error("server protocol too old for client {client_version}; max {server_max}")]
    ServerTooOld { client_version: u16, server_max: u16 },
    #[error("incompatible protocol: {reason}")]
    Incompatible { reason: String },
    #[error("required feature missing on client: {0}")]
    RequiredFeatureMissing(FeatureId),
    #[error("hello malformed: {0}")]
    Malformed(String),
}

/// Run the server-side negotiation. Never trusts the client's self-reported
/// version of the negotiated set — the server computes `allowed`.
pub fn negotiate(policy: &NegotiationPolicy, client: &Hello) -> Result<NegotiatedSet, NegotiationError> {
    let mut problems: Vec<String> = Vec::new();

    // --- protocol version ---
    let protocol = negotiate_protocol(policy, client)?;

    // --- features ---
    let mut features: BTreeMap<String, FeatureOutcome> = BTreeMap::new();

    // everything the client asks about
    for cf in &client.features {
        let key = cf.key();
        if policy.disabled.iter().any(|d| d == &key || d == &format!("{}:{}", cf.namespace, cf.name)) {
            features.insert(key, FeatureOutcome::DisabledByPolicy(cf.clone()));
            continue;
        }
        match policy.server_features.iter().find(|sf| sf.same_feature(cf)) {
            None => {
                features.insert(key, FeatureOutcome::ClientOnlyUnsupported(cf.clone()));
            }
            Some(sf) => {
                if sf.version >= cf.version {
                    // server can serve the client's (older-or-equal) variant
                    features.insert(key, FeatureOutcome::Selected(cf.clone()));
                } else {
                    // server older than client wants
                    features.insert(key, FeatureOutcome::RequiresServerUpdate(sf.clone()));
                    problems.push(format!("feature {cf} needs server update"));
                }
            }
        };
    }

    // server-only features the client is too old for
    for sf in &policy.server_features {
        if !client.features.iter().any(|cf| cf.same_feature(sf)) {
            // Not requested. If strictly required, that's a client gap.
            if policy.strict_required_features.iter().any(|r| r.same_feature(sf)) {
                return Err(NegotiationError::RequiredFeatureMissing(sf.clone()));
            }
            features.insert(sf.key(), FeatureOutcome::RequiresClientUpdate(sf.clone()));
        }
    }

    // --- capabilities ---
    // Intersection of what the client advertises and what the server can serve.
    // A capability is free-form ("voice-opus"); it is "shared" only when the
    // server explicitly offers it OR offers a feature of the same name.
    let server_cap_names: std::collections::HashSet<&str> = policy
        .server_capabilities
        .iter()
        .map(|c| c.as_str())
        .chain(policy.server_features.iter().map(|f| f.name.as_str()))
        .collect();
    let mut shared: Vec<String> =
        client.capabilities.iter().filter(|c| server_cap_names.contains(c.as_str())).cloned().collect();
    shared.sort_unstable();
    shared.dedup();

    let allowed = matches!(protocol, ProtocolNegotiation::Agreed { .. }) && problems.is_empty();

    let summary = if allowed {
        format!(
            "protocol v{} agreed; {} features selected; {} shared capabilities",
            PROTOCOL_VERSION,
            features.values().filter(|o| matches!(o, FeatureOutcome::Selected(_))).count(),
            shared.len()
        )
    } else {
        format!(
            "negotiation failed: {} problems; first: {}",
            problems.len(),
            problems.first().cloned().unwrap_or_default()
        )
    };

    Ok(NegotiatedSet { protocol, features, shared_capabilities: shared, allowed, summary })
}

fn negotiate_protocol(policy: &NegotiationPolicy, client: &Hello) -> Result<ProtocolNegotiation, NegotiationError> {
    if client.protocol_max < policy.server_protocol_min {
        let action = UpgradeAction::UpdateClient {
            channel: policy.client_channel.clone(),
            min_version: policy.client_min_version.clone(),
        };
        return Ok(ProtocolNegotiation::ClientTooOld {
            client_version: client.protocol_max,
            server_min: policy.server_protocol_min,
            server_max: policy.server_protocol_max,
            action,
        });
    }

    if client.protocol_min > policy.server_protocol_max {
        let action = UpgradeAction::UpdateServer {
            channel: policy.server_channel.clone(),
            min_version: policy.server_min_version.clone(),
        };
        return Ok(ProtocolNegotiation::ServerTooOld {
            client_version: client.protocol_min,
            server_max: policy.server_protocol_max,
            action,
        });
    }

    // Highest version both sides speak.
    let common = policy.server_protocol_max.min(client.protocol_max);
    if common < policy.server_protocol_min.min(client.protocol_max) {
        return Ok(ProtocolNegotiation::Incompatible {
            reason: "no overlapping protocol version range".to_string(),
            action: UpgradeAction::UnsupportedCombination,
        });
    }

    Ok(ProtocolNegotiation::Agreed { version: common })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> NegotiationPolicy {
        NegotiationPolicy {
            server_features: vec![
                FeatureId::new("ald", "statebags", 2),
                FeatureId::new("ald", "voice", 1),
                FeatureId::new("ald", "dui", 1),
            ],
            server_capabilities: vec!["voice-opus".to_string()],
            ..Default::default()
        }
    }

    fn client_hello() -> Hello {
        Hello::new("astryn", "1.0.0")
            .with_feature(FeatureId::new("ald", "statebags", 2))
            .with_feature(FeatureId::new("ald", "voice", 1))
            .with_capability("voice-opus")
    }

    #[test]
    fn feature_key_roundtrips() {
        let f = FeatureId::new("ald", "statebags", 2);
        assert_eq!(f.key(), "ald:statebags@2");
        let g = FeatureId::new("ald", "statebags", 5);
        assert!(f.same_feature(&g));
        assert!(!f.same_feature(&FeatureId::new("ald", "voice", 2)));
    }

    #[test]
    fn clean_handshake_agrees() {
        let set = negotiate(&policy(), &client_hello()).unwrap();
        assert!(set.allowed);
        assert!(matches!(set.protocol, ProtocolNegotiation::Agreed { version: 1 }));
        assert!(matches!(set.features.get("ald:statebags@2"), Some(FeatureOutcome::Selected(_))));
        assert!(set.shared_capabilities.contains(&"voice-opus".to_string()));
    }

    #[test]
    fn unknown_client_feature_marked_not_selected() {
        let mut c = client_hello();
        c.features.push(FeatureId::new("acme", "teleport", 1));
        let set = negotiate(&policy(), &c).unwrap();
        assert!(matches!(set.features.get("acme:teleport@1"), Some(FeatureOutcome::ClientOnlyUnsupported(_))));
    }

    #[test]
    fn disabled_feature_blocks_selection() {
        let mut p = policy();
        p.disabled.push("ald:voice".to_string());
        let set = negotiate(&p, &client_hello()).unwrap();
        assert!(matches!(set.features.get("ald:voice@1"), Some(FeatureOutcome::DisabledByPolicy(_))));
    }

    #[test]
    fn server_feature_client_lacks_is_client_update() {
        let set = negotiate(&policy(), &client_hello()).unwrap();
        assert!(matches!(set.features.get("ald:dui@1"), Some(FeatureOutcome::RequiresClientUpdate(_))));
    }

    #[test]
    fn client_newer_than_server_feature_is_server_update() {
        let mut c = client_hello();
        c.features.push(FeatureId::new("ald", "statebags", 9));
        let set = negotiate(&policy(), &c).unwrap();
        assert!(!set.allowed);
        assert!(matches!(set.features.get("ald:statebags@9"), Some(FeatureOutcome::RequiresServerUpdate(_))));
    }

    #[test]
    fn required_feature_missing_hard_fails() {
        let mut p = policy();
        p.strict_required_features.push(FeatureId::new("ald", "dui", 1));
        let err = negotiate(&p, &client_hello()).unwrap_err();
        assert!(matches!(err, NegotiationError::RequiredFeatureMissing(_)));
    }

    #[test]
    fn client_protocol_too_old_produces_structured_outcome() {
        let mut c = client_hello();
        c.protocol_max = 0;
        c.protocol_min = 0;
        let set = negotiate(&policy(), &c).unwrap();
        match set.protocol {
            ProtocolNegotiation::ClientTooOld { action, .. } => {
                assert!(matches!(action, UpgradeAction::UpdateClient { .. }));
            }
            other => panic!("expected ClientTooOld, got {other:?}"),
        }
        assert!(!set.allowed);
    }

    #[test]
    fn server_protocol_too_old_produces_structured_outcome() {
        let mut c = client_hello();
        c.protocol_min = 99;
        let set = negotiate(&policy(), &c).unwrap();
        match set.protocol {
            ProtocolNegotiation::ServerTooOld { action, .. } => {
                assert!(matches!(action, UpgradeAction::UpdateServer { .. }));
            }
            other => panic!("expected ServerTooOld, got {other:?}"),
        }
        assert!(!set.allowed);
    }

    #[test]
    fn incompatible_when_no_overlap() {
        let mut p = policy();
        p.server_protocol_max = 5;
        p.server_protocol_min = 5;
        let mut c = client_hello();
        c.protocol_min = 1;
        c.protocol_max = 3;
        let set = negotiate(&p, &c).unwrap();
        assert!(matches!(set.protocol, ProtocolNegotiation::ClientTooOld { .. }));
    }

    #[test]
    fn negotiated_set_serde_roundtrip() {
        let set = negotiate(&policy(), &client_hello()).unwrap();
        let j = serde_json::to_string(&set).unwrap();
        let back: NegotiatedSet = serde_json::from_str(&j).unwrap();
        assert_eq!(back.allowed, set.allowed);
        assert_eq!(back.features.len(), set.features.len());
    }
}
