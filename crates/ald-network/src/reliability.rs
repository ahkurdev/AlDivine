//! Reliability layer: duplicate detection, replay protection, ordering.
//! Per-session sequence window. Out-of-order packets are accepted but
//! duplicates and replays are rejected.

/// Window size for received sequence numbers.
const WINDOW: u32 = 1024;

/// State for validating incoming packet sequence numbers.
#[derive(Debug)]
pub struct ReliabilityState {
    /// Highest contiguous sequence number accepted so far.
    highest: u32,
    seen: bool,
    /// Bitmask of recently accepted sequences relative to `highest`
    /// (bit i set => sequence highest - i accepted). Allows dup detection
    /// within the window without storing every id.
    mask: u64,
}

impl Default for ReliabilityState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReliabilityState {
    pub fn new() -> Self {
        ReliabilityState { highest: 0, seen: false, mask: 0 }
    }

    /// Returns Ok(()) if the sequence is new (not a duplicate/replay),
    /// Err otherwise. Sequences may arrive out of order.
    ///
    /// `mask` bit i means sequence `highest - i` was accepted (i < 64).
    /// Forward distance is computed modulo 2^32 so the 32-bit sequence space
    /// can wrap safely.
    pub fn validate(&mut self, seq: u32) -> Result<(), ReliabilityError> {
        if !self.seen {
            self.highest = seq;
            self.seen = true;
            self.mask = 1;
            return Ok(());
        }

        if seq == self.highest {
            return Err(ReliabilityError::Duplicate);
        }

        let forward = seq.wrapping_sub(self.highest);
        let half: u32 = 1u32 << 31;

        if forward < half {
            // Packet is ahead of the highest seen sequence.
            if forward < 64 {
                // Shift the window so bit i means (new highest - i); the old
                // highest lands at distance `forward`. Duplicates ahead of the
                // highest are impossible (they'd be the highest itself).
                self.mask = (self.mask << forward) | 1;
                self.highest = seq;
            } else {
                // Beyond the 64-slot mask or a large jump: reset the window.
                self.mask = 1;
                self.highest = seq;
            }
            return Ok(());
        }

        // Packet is behind the highest seen sequence.
        let back = self.highest.wrapping_sub(seq);
        if back >= WINDOW {
            return Err(ReliabilityError::Stale);
        }
        if back < 64 {
            let bit = 1u64 << back;
            if self.mask & bit != 0 {
                return Err(ReliabilityError::Duplicate);
            }
            self.mask |= bit;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReliabilityError {
    Duplicate,
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_packet_ok() {
        let mut r = ReliabilityState::new();
        assert!(r.validate(100).is_ok());
    }

    #[test]
    fn exact_duplicate_rejected() {
        let mut r = ReliabilityState::new();
        r.validate(100).unwrap();
        assert_eq!(r.validate(100), Err(ReliabilityError::Duplicate));
    }

    #[test]
    fn out_of_order_then_dup_rejected() {
        let mut r = ReliabilityState::new();
        r.validate(100).unwrap();
        assert!(r.validate(103).is_ok()); // ahead
        assert!(r.validate(101).is_ok()); // gap fill
        assert_eq!(r.validate(101), Err(ReliabilityError::Duplicate));
    }

    #[test]
    fn far_stale_rejected() {
        let mut r = ReliabilityState::new();
        r.validate(1_000_000).unwrap();
        assert_eq!(r.validate(500_000), Err(ReliabilityError::Stale));
    }

    #[test]
    fn sequence_wraps() {
        let mut r = ReliabilityState::new();
        r.validate(u32::MAX - 1).unwrap();
        assert!(r.validate(u32::MAX).is_ok());
        assert!(r.validate(0).is_ok()); // wrap
    }
}
