//! Lightweight metrics registry. Prometheus-compatible exposition planned;
//! this core provides lock-free-ish counters/histograms collected per resource.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

    /// Increment counter only if caller has provided explicit telemetry consent.
    pub fn incr_with_consent(&self, name: &str, by: u64, consent: &TelemetryConsent) -> bool {
        if consent.allows(TelemetryCategory::PerformanceMetrics) {
            self.incr(name, by);
            true
        } else {
            false
        }
    }
}

/// Granular categories of telemetry signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TelemetryCategory {
    PerformanceMetrics,
    CrashReporting,
    UsageAnalytics,
    NetworkDiagnostics,
}

/// Explicit player/operator telemetry consent state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryConsent {
    Unset,
    OptedOut,
    OptedIn { categories: HashSet<TelemetryCategory> },
}

impl TelemetryConsent {
    /// Default-deny check: allows collection only if explicitly opted-in to the specific category.
    pub fn allows(&self, category: TelemetryCategory) -> bool {
        match self {
            Self::OptedIn { categories } => categories.contains(&category),
            Self::Unset | Self::OptedOut => false,
        }
    }
}

/// Accessibility preferences across NovaGate launcher, in-game console, and UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessibilitySettings {
    pub high_contrast: bool,
    pub text_scale: f32,
    pub reduced_motion: bool,
    pub screen_reader_hints: bool,
    pub colorblind_mode: ColorblindMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorblindMode {
    None,
    Protanopia,
    Deuteranopia,
    Tritanopia,
}

impl Default for AccessibilitySettings {
    fn default() -> Self {
        Self {
            high_contrast: false,
            text_scale: 1.0,
            reduced_motion: false,
            screen_reader_hints: false,
            colorblind_mode: ColorblindMode::None,
        }
    }
}

/// Multi-locale string catalog supporting variable interpolation and fallback chains.
#[derive(Debug, Clone, Default)]
pub struct LocalizationCatalog {
    strings: HashMap<(String, String), String>, // (locale, key) -> template
    default_locale: String,
}

impl LocalizationCatalog {
    pub fn new(default_locale: impl Into<String>) -> Self {
        Self { strings: HashMap::new(), default_locale: default_locale.into() }
    }

    pub fn insert(&mut self, locale: &str, key: &str, template: &str) {
        self.strings.insert((locale.to_string(), key.to_string()), template.to_string());
    }

    pub fn get(&self, locale: &str, key: &str, vars: &HashMap<&str, &str>) -> String {
        let template = self
            .strings
            .get(&(locale.to_string(), key.to_string()))
            .or_else(|| self.strings.get(&(self.default_locale.clone(), key.to_string())));

        let Some(t) = template else {
            return key.to_string(); // Fallback to key verbatim
        };

        let mut rendered = t.clone();
        for (&k, &v) in vars {
            rendered = rendered.replace(&format!("{{{}}}", k), v);
        }
        rendered
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

    #[test]
    fn consent_default_deny_blocks_collection() {
        let m = MetricStore::new();
        let consent = TelemetryConsent::Unset;

        assert!(!consent.allows(TelemetryCategory::PerformanceMetrics));
        assert!(!m.incr_with_consent("frame_time", 1, &consent));
        assert_eq!(m.counter("frame_time"), 0);
    }

    #[test]
    fn consent_opt_in_allows_specified_category() {
        let m = MetricStore::new();
        let mut cats = HashSet::new();
        cats.insert(TelemetryCategory::PerformanceMetrics);
        let consent = TelemetryConsent::OptedIn { categories: cats };

        assert!(consent.allows(TelemetryCategory::PerformanceMetrics));
        assert!(!consent.allows(TelemetryCategory::CrashReporting));

        assert!(m.incr_with_consent("frame_time", 1, &consent));
        assert_eq!(m.counter("frame_time"), 1);
    }

    #[test]
    fn consent_opt_out_blocks_all() {
        let consent = TelemetryConsent::OptedOut;
        assert!(!consent.allows(TelemetryCategory::PerformanceMetrics));
        assert!(!consent.allows(TelemetryCategory::CrashReporting));
        assert!(!consent.allows(TelemetryCategory::UsageAnalytics));
        assert!(!consent.allows(TelemetryCategory::NetworkDiagnostics));
    }

    #[test]
    fn localization_retrieves_exact_locale_and_interpolates() {
        let mut cat = LocalizationCatalog::new("en-US");
        cat.insert("en-US", "welcome", "Welcome, {user}!");
        cat.insert("id-ID", "welcome", "Selamat datang, {user}!");

        let mut vars = HashMap::new();
        vars.insert("user", "Allan");

        assert_eq!(cat.get("en-US", "welcome", &vars), "Welcome, Allan!");
        assert_eq!(cat.get("id-ID", "welcome", &vars), "Selamat datang, Allan!");
    }

    #[test]
    fn localization_falls_back_to_default_locale() {
        let mut cat = LocalizationCatalog::new("en-US");
        cat.insert("en-US", "quit", "Quit Server");

        let vars = HashMap::new();
        assert_eq!(cat.get("de-DE", "quit", &vars), "Quit Server");
        assert_eq!(cat.get("de-DE", "nonexistent_key", &vars), "nonexistent_key");
    }

    #[test]
    fn accessibility_default_settings() {
        let acc = AccessibilitySettings::default();
        assert!(!acc.high_contrast);
        assert_eq!(acc.text_scale, 1.0);
        assert_eq!(acc.colorblind_mode, ColorblindMode::None);
    }
}
