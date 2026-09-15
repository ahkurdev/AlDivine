/// Monotonic-ish wall-clock helpers in milliseconds since UNIX epoch.
/// Used for packet timestamps, ticks, and audit records.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn now_secs() -> u64 {
    now_ms() / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_increases() {
        let a = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = now_ms();
        assert!(b >= a);
    }
}
