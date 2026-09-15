//! Per-peer rate limiting / anti-flood. Bounded token bucket: packets above
//! the configured burst rate are dropped before any gameplay code runs.

use std::time::Instant;

/// Token-bucket rate limiter. Capacity = burst; refill = tokens per second.
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(capacity: u32, refill_per_sec: f64) -> Self {
        RateLimiter { capacity: capacity as f64, refill_per_sec, tokens: capacity as f64, last: Instant::now() }
    }

    /// Try to consume one token. Returns false when rate exceeded.
    pub fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn tokens(&self) -> f64 {
        self.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_deny() {
        let mut rl = RateLimiter::new(3, 0.0); // no refill -> pure burst
        assert!(rl.try_consume());
        assert!(rl.try_consume());
        assert!(rl.try_consume());
        assert!(!rl.try_consume());
    }

    #[test]
    fn refills_over_time() {
        let mut rl = RateLimiter::new(1, 1000.0); // fast refill
        assert!(rl.try_consume());
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(rl.try_consume());
    }
}
