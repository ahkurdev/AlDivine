//! Lightweight metrics registry. Prometheus-compatible exposition planned;
//! this core provides lock-free-ish counters/histograms collected per resource.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Default)]
pub struct MetricStore {
    counters: Mutex<HashMap<String, u64>>,
    gauges: Mutex<HashMap<String, f64>>,
    /// record of sample times for simple latency histograms
    timings: Mutex<HashMap<String, Vec<u64>>>,
}

impl MetricStore {
    pub fn new() -> Self {
        MetricStore::default()
    }

    pub fn incr(&self, name: &str, by: u64) {
        let mut c = self.counters.lock().unwrap();
        *c.entry(name.to_string()).or_insert(0) += by;
    }

    pub fn gauge(&self, name: &str, value: f64) {
        let mut g = self.gauges.lock().unwrap();
        g.insert(name.to_string(), value);
    }

    pub fn record(&self, name: &str, micros: u64) {
        let mut t = self.timings.lock().unwrap();
        t.entry(name.to_string()).or_default().push(micros);
        // Bound history to avoid unbounded growth.
        let v = t.get_mut(name).unwrap();
        if v.len() > 1024 {
            v.drain(0..512);
        }
    }

    /// Percentile (0..=100) over recorded samples. Returns 0 if empty.
    pub fn percentile(&self, name: &str, pct: u8) -> u64 {
        let t = self.timings.lock().unwrap();
        let samples = match t.get(name) {
            Some(s) if !s.is_empty() => s,
            _ => return 0,
        };
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        let idx = ((pct.min(100) as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[idx]
    }

    pub fn counter(&self, name: &str) -> u64 {
        *self.counters.lock().unwrap().get(name).unwrap_or(&0)
    }

    pub fn gauge_val(&self, name: &str) -> f64 {
        *self.gauges.lock().unwrap().get(name).unwrap_or(&0.0)
    }
}

/// Convenience RAII timer that records elapsed micros on drop.
pub struct Timer<'a> {
    store: &'a MetricStore,
    name: String,
    start: Instant,
}

impl<'a> Timer<'a> {
    pub fn start(store: &'a MetricStore, name: &str) -> Self {
        Timer { store, name: name.to_string(), start: Instant::now() }
    }
}

impl<'a> Drop for Timer<'a> {
    fn drop(&mut self) {
        let micros = self.start.elapsed().as_micros() as u64;
        self.store.record(&self.name, micros);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_and_gauge() {
        let m = MetricStore::new();
        m.incr("events", 3);
        m.incr("events", 2);
        assert_eq!(m.counter("events"), 5);
        m.gauge("players", 32.0);
        assert_eq!(m.gauge_val("players"), 32.0);
    }

    #[test]
    fn percentile_basic() {
        let m = MetricStore::new();
        for i in 1..=11u64 {
            m.record("lat", i * 10);
        }
        assert_eq!(m.percentile("lat", 50), 60);
        assert_eq!(m.percentile("lat", 0), 10);
        assert_eq!(m.percentile("lat", 100), 110);
    }

    #[test]
    fn timer_records() {
        let m = MetricStore::new();
        {
            let _t = Timer::start(&m, "op");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(m.percentile("op", 100) >= 1000);
    }
}
