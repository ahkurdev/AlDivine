//! Voice groundwork: channels, proximity tiers, metadata envelopes, mutes.
//!
//! Audio capture, encoding, and transport are an allowed native boundary
//! that is NOT built here and not claimed. What this crate proves is the
//! policy and routing logic around it:
//!
//! - [`VoiceChannel`]: Proximity (positional broadcast), Radio (frequency),
//!   Direct (member list). Membership is bounded and explicit; joining a
//!   full channel fails instead of silently dropping someone else.
//! - Proximity tiers ([`proximity_tier`]): Audible / Clear / Faint /
//!   OutOfRange by distance, so the audio layer knows who hears whom at
//!   what quality without re-deriving radii.
//! - [`VoiceEnvelope`]: speaker + channel + sequence + talking flag +
//!   priority. Bounded payloads (metadata only — no audio bytes flow
//!   through here).
//! - Server mutes: [`VoiceRouter::may_transmit`] drops muted speakers
//!   before any routing decision. Deafened listeners receive nothing.
//!
//! The audio pipeline consumes validated envelopes and tier decisions
//! through the bridge later.

use std::collections::{HashMap, HashSet};

use thiserror::Error;

/// Audible radius tiers (world units, horizontal plane like interest).
pub const RADIUS_CLEAR: f32 = 16.0;
pub const RADIUS_AUDIBLE: f32 = 48.0;
pub const RADIUS_FAINT: f32 = 96.0;

/// How well `listener` hears a speaker at `distance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProximityTier {
    Clear,
    Audible,
    Faint,
    OutOfRange,
}

pub fn proximity_tier(distance: f32) -> ProximityTier {
    if distance <= RADIUS_CLEAR {
        ProximityTier::Clear
    } else if distance <= RADIUS_AUDIBLE {
        ProximityTier::Audible
    } else if distance <= RADIUS_FAINT {
        ProximityTier::Faint
    } else {
        ProximityTier::OutOfRange
    }
}

/// Channel kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    /// Positional broadcast within radii.
    Proximity,
    /// Named frequency (e.g. police radio).
    Radio { frequency: String },
    /// Explicit member list (phone call).
    Direct,
}

/// A voice channel with bounded membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceChannel {
    pub id: String,
    pub kind: ChannelKind,
    pub max_members: usize,
    pub members: HashSet<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VoiceError {
    #[error("unknown channel '{0}'")]
    UnknownChannel(String),
    #[error("bad channel id '{0}'")]
    BadId(String),
    #[error("bad frequency '{0}'")]
    BadFrequency(String),
    #[error("channel '{0}' is full ({1} members)")]
    ChannelFull(String, usize),
    #[error("metadata too large ({0} bytes)")]
    MetadataTooLarge(usize),
}

pub const MAX_METADATA_LEN: usize = 4096;

/// Voice metadata envelope (no audio bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceEnvelope {
    pub speaker: u32,
    pub channel: String,
    pub seq: u32,
    pub talking: bool,
    pub priority: bool,
    pub metadata: Vec<u8>,
}

impl VoiceEnvelope {
    pub fn new(
        speaker: u32,
        channel: &str,
        seq: u32,
        talking: bool,
        priority: bool,
        metadata: Vec<u8>,
    ) -> Result<Self, VoiceError> {
        if metadata.len() > MAX_METADATA_LEN {
            return Err(VoiceError::MetadataTooLarge(metadata.len()));
        }
        Ok(VoiceEnvelope { speaker, channel: channel.to_string(), seq, talking, priority, metadata })
    }
}

/// Channel registry plus server mute/deafen policy.
#[derive(Default)]
pub struct VoiceRouter {
    channels: HashMap<String, VoiceChannel>,
    muted: HashSet<u32>,
    deafened: HashSet<u32>,
}

impl VoiceRouter {
    pub fn new() -> Self {
        VoiceRouter::default()
    }

    fn valid_id(id: &str) -> Result<(), VoiceError> {
        if id.is_empty()
            || id.len() > 64
            || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b':')
        {
            return Err(VoiceError::BadId(id.to_string()));
        }
        Ok(())
    }

    /// Create a channel. Radio frequencies are strict tokens.
    pub fn create(&mut self, id: &str, kind: ChannelKind, max_members: usize) -> Result<(), VoiceError> {
        Self::valid_id(id)?;
        if let ChannelKind::Radio { frequency } = &kind {
            if frequency.is_empty()
                || frequency.len() > 32
                || !frequency.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.')
            {
                return Err(VoiceError::BadFrequency(frequency.clone()));
            }
        }
        if self.channels.contains_key(id) {
            return Err(VoiceError::BadId(format!("duplicate channel {id}")));
        }
        self.channels.insert(
            id.to_string(),
            VoiceChannel { id: id.to_string(), kind, max_members: max_members.max(1), members: HashSet::new() },
        );
        Ok(())
    }

    pub fn drop_channel(&mut self, id: &str) -> bool {
        self.channels.remove(id).is_some()
    }

    /// Join a channel. Full channels refuse; members re-joining are idempotent.
    pub fn join(&mut self, channel: &str, player: u32) -> Result<(), VoiceError> {
        let ch = self.channels.get_mut(channel).ok_or_else(|| VoiceError::UnknownChannel(channel.to_string()))?;
        if ch.members.len() >= ch.max_members && !ch.members.contains(&player) {
            return Err(VoiceError::ChannelFull(channel.to_string(), ch.max_members));
        }
        ch.members.insert(player);
        Ok(())
    }

    pub fn leave(&mut self, channel: &str, player: u32) -> Result<(), VoiceError> {
        let ch = self.channels.get_mut(channel).ok_or_else(|| VoiceError::UnknownChannel(channel.to_string()))?;
        ch.members.remove(&player);
        Ok(())
    }

    pub fn members(&self, channel: &str) -> Option<Vec<u32>> {
        let mut v: Vec<u32> = self.channels.get(channel)?.members.iter().copied().collect();
        v.sort_unstable();
        Some(v)
    }

    pub fn mute(&mut self, player: u32, muted: bool) {
        if muted {
            self.muted.insert(player);
        } else {
            self.muted.remove(&player);
        }
    }

    pub fn deafen(&mut self, player: u32, deafened: bool) {
        if deafened {
            self.deafened.insert(player);
        } else {
            self.deafened.remove(&player);
        }
    }

    /// May this envelope transmit? Muted speakers never; deafened listeners
    /// are filtered at fan-out (see `recipients`).
    pub fn may_transmit(&self, env: &VoiceEnvelope) -> bool {
        env.talking && !self.muted.contains(&env.speaker)
    }

    /// Recipients for an envelope: channel members minus the speaker,
    /// minus deafened listeners. Unknown channels yield empty (no fan-out
    /// to nowhere, no error — the producer re-checks membership).
    pub fn recipients(&self, env: &VoiceEnvelope) -> Vec<u32> {
        match self.channels.get(&env.channel) {
            None => Vec::new(),
            Some(ch) => {
                let mut v: Vec<u32> =
                    ch.members.iter().filter(|p| **p != env.speaker && !self.deafened.contains(p)).copied().collect();
                v.sort_unstable();
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proximity_tiers_by_distance() {
        assert_eq!(proximity_tier(5.0), ProximityTier::Clear);
        assert_eq!(proximity_tier(16.0), ProximityTier::Clear);
        assert_eq!(proximity_tier(30.0), ProximityTier::Audible);
        assert_eq!(proximity_tier(48.0), ProximityTier::Audible);
        assert_eq!(proximity_tier(90.0), ProximityTier::Faint);
        assert_eq!(proximity_tier(96.0), ProximityTier::Faint);
        assert_eq!(proximity_tier(200.0), ProximityTier::OutOfRange);
    }

    #[test]
    fn channel_lifecycle_and_caps() {
        let mut r = VoiceRouter::new();
        r.create("prox", ChannelKind::Proximity, 64).unwrap();
        assert_eq!(
            r.create("prox", ChannelKind::Proximity, 64),
            Err(VoiceError::BadId("duplicate channel prox".into()))
        );
        assert!(r.create("", ChannelKind::Proximity, 4).is_err());
        assert!(r.create("bad id", ChannelKind::Proximity, 4).is_err());
        assert!(r.join("prox", 1).is_ok());
        assert!(r.join("prox", 1).is_ok()); // idempotent re-join
        assert_eq!(r.members("prox"), Some(vec![1]));
        assert_eq!(r.members("ghost"), None);
        assert!(r.join("ghost", 1).is_err());
        r.leave("prox", 1).unwrap();
        assert_eq!(r.members("prox"), Some(vec![]));
        assert!(r.drop_channel("prox"));
        assert!(!r.drop_channel("prox"));
    }

    #[test]
    fn full_channels_refuse() {
        let mut r = VoiceRouter::new();
        r.create("call", ChannelKind::Direct, 2).unwrap();
        r.join("call", 1).unwrap();
        r.join("call", 2).unwrap();
        assert_eq!(r.join("call", 3), Err(VoiceError::ChannelFull("call".into(), 2)));
        // Leaving frees the seat.
        r.leave("call", 1).unwrap();
        r.join("call", 3).unwrap();
    }

    #[test]
    fn radio_frequencies_validated() {
        let mut r = VoiceRouter::new();
        r.create("pd", ChannelKind::Radio { frequency: "421.5".into() }, 16).unwrap();
        for bad in ["", "bad freq!", &"f".repeat(33)] {
            assert_eq!(
                r.create("x", ChannelKind::Radio { frequency: bad.into() }, 16),
                Err(VoiceError::BadFrequency(bad.into()))
            );
        }
    }

    #[test]
    fn envelopes_bounded() {
        let e = VoiceEnvelope::new(1, "prox", 0, true, false, vec![0; MAX_METADATA_LEN]).unwrap();
        assert!(e.talking && !e.priority);
        assert_eq!(
            VoiceEnvelope::new(1, "prox", 0, true, false, vec![0; MAX_METADATA_LEN + 1]),
            Err(VoiceError::MetadataTooLarge(MAX_METADATA_LEN + 1))
        );
    }

    #[test]
    fn mutes_and_deafens_shape_routing() {
        let mut r = VoiceRouter::new();
        r.create("prox", ChannelKind::Proximity, 8).unwrap();
        for p in [1, 2, 3] {
            r.join("prox", p).unwrap();
        }
        let env = VoiceEnvelope::new(1, "prox", 0, true, false, vec![]).unwrap();
        assert!(r.may_transmit(&env));
        assert_eq!(r.recipients(&env), vec![2, 3]); // speaker excluded
        r.mute(1, true);
        assert!(!r.may_transmit(&env));
        r.mute(1, false);
        assert!(r.may_transmit(&env));
        // Silent (talking=false) envelopes never transmit.
        let quiet = VoiceEnvelope::new(1, "prox", 1, false, false, vec![]).unwrap();
        assert!(!r.may_transmit(&quiet));
        r.deafen(2, true);
        assert_eq!(r.recipients(&env), vec![3]);
        r.deafen(2, false);
        assert_eq!(r.recipients(&env), vec![2, 3]);
        // Unknown channel: no fan-out.
        let lost = VoiceEnvelope::new(1, "ghost", 0, true, false, vec![]).unwrap();
        assert!(r.recipients(&lost).is_empty());
    }

    #[test]
    fn priority_flag_carried() {
        let e = VoiceEnvelope::new(9, "pd", 4, true, true, b"10-78".to_vec()).unwrap();
        assert!(e.priority);
        assert_eq!(e.seq, 4);
    }
}
