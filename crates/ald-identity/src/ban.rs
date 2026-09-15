use serde::{Deserialize, Serialize};

/// Ban matching with confidence-scored signals.
/// IP alone is a weak signal (NAT/CGNAT/VPN). Steam/RS verified are strong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BanMatchSignal {
    AldivineAccount,
    RockstarVerified,
    SteamVerified,
    EpicVerified,
    DeviceId,
    IpAddress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanMatchResult {
    pub confidence: f32,
    pub matched_signals: Vec<BanMatchSignal>,
    pub rule: String,
}

impl BanMatchResult {
    pub fn new(rule: impl Into<String>) -> Self {
        BanMatchResult { confidence: 0.0, matched_signals: Vec::new(), rule: rule.into() }
    }

    pub fn add_signal(&mut self, signal: BanMatchSignal, weight: f32) {
        self.matched_signals.push(signal);
        self.confidence = (self.confidence + weight).min(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_accumulates_and_caps() {
        let mut r = BanMatchResult::new("abuse-policy");
        r.add_signal(BanMatchSignal::RockstarVerified, 0.6);
        r.add_signal(BanMatchSignal::DeviceId, 0.2);
        assert_eq!(r.confidence, 0.8);
        r.add_signal(BanMatchSignal::IpAddress, 0.5);
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.matched_signals.len(), 3);
    }
}
