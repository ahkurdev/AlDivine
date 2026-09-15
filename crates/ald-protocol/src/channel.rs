use serde::{Deserialize, Serialize};

/// Logical AstraNet channels. Each selects a transport and QoS policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Channel {
    Control = 0,
    Auth = 1,
    EventReliable = 2,
    EventUnreliable = 3,
    EntityState = 4,
    ResourceTransfer = 5,
    Admin = 6,
    Heartbeat = 7,
    VoiceMetadata = 8,
}

impl Channel {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Channel::Control),
            1 => Some(Channel::Auth),
            2 => Some(Channel::EventReliable),
            3 => Some(Channel::EventUnreliable),
            4 => Some(Channel::EntityState),
            5 => Some(Channel::ResourceTransfer),
            6 => Some(Channel::Admin),
            7 => Some(Channel::Heartbeat),
            8 => Some(Channel::VoiceMetadata),
            _ => None,
        }
    }

    /// Unreliable channels are sent over UDP datagrams or QUIC datagrams.
    pub fn is_unreliable(&self) -> bool {
        matches!(self, Channel::EventUnreliable | Channel::EntityState | Channel::VoiceMetadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_roundtrip() {
        for c in [Channel::Control, Channel::Auth, Channel::EntityState, Channel::VoiceMetadata] {
            assert_eq!(Channel::from_u8(c as u8), Some(c));
        }
        assert_eq!(Channel::from_u8(99), None);
    }
}
