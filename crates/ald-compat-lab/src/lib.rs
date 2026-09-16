//! Aldivine Compatibility Lab & Certification Suite
//!
//! Enforces the single source of truth rule for compatibility certification:
//! - Exact matrix evaluation over synthetic fixtures and legal real-world OSS resources
//! - Strict definition of "100% Supported": requires 100% PASS on all certified gates
//! - Honest reporting: any failed or skipped gate rejects 100% certification claim

use serde::{Deserialize, Serialize};

/// Result of an individual gate evaluation within the certification matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateResult {
    Pass,
    Fail,
    Skipped,
}

impl GateResult {
    pub fn is_pass(self) -> bool {
        self == GateResult::Pass
    }
}

/// Exact certified support matrix required for 100% certification claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedSupportMatrix {
    pub matrix_id: String,
    pub runtime_target: String,
    pub gta_legacy_build_2699: GateResult,
    pub gta_enhanced_build_3095: GateResult,
    pub cfxlua_profile: GateResult,
    pub native_js: GateResult,
    pub node16: GateResult,
    pub node22: GateResult,
    pub clr: GateResult,
    pub fxmanifest_supported_set: GateResult,
    pub citizen_required_corpus: GateResult,
    pub native_required_corpus: GateResult,
    pub asset_corpus: GateResult,
    pub datafile_corpus: GateResult,
    pub postgresql: GateResult,
    pub mysql: GateResult,
    pub mariadb: GateResult,
    pub esx_compat: GateResult,
    pub qbcore_compat: GateResult,
    pub qbox_compat: GateResult,
    pub streaming_chaos: GateResult,
    pub windows_build: GateResult,
    pub native_dependency_audit: GateResult,
}

impl CertifiedSupportMatrix {
    /// Return all gate names and their results.
    pub fn all_gates(&self) -> Vec<(&'static str, GateResult)> {
        vec![
            ("gta_legacy_build_2699", self.gta_legacy_build_2699),
            ("gta_enhanced_build_3095", self.gta_enhanced_build_3095),
            ("cfxlua_profile", self.cfxlua_profile),
            ("native_js", self.native_js),
            ("node16", self.node16),
            ("node22", self.node22),
            ("clr", self.clr),
            ("fxmanifest_supported_set", self.fxmanifest_supported_set),
            ("citizen_required_corpus", self.citizen_required_corpus),
            ("native_required_corpus", self.native_required_corpus),
            ("asset_corpus", self.asset_corpus),
            ("datafile_corpus", self.datafile_corpus),
            ("postgresql", self.postgresql),
            ("mysql", self.mysql),
            ("mariadb", self.mariadb),
            ("esx_compat", self.esx_compat),
            ("qbcore_compat", self.qbcore_compat),
            ("qbox_compat", self.qbox_compat),
            ("streaming_chaos", self.streaming_chaos),
            ("windows_build", self.windows_build),
            ("native_dependency_audit", self.native_dependency_audit),
        ]
    }

    /// Strictly evaluate whether this matrix qualifies for a 100% Certified claim.
    /// If even a single gate is not Pass, returns false.
    pub fn is_100_percent_certified(&self) -> bool {
        self.all_gates().iter().all(|(_, res)| res.is_pass())
    }

    /// List all gates that are blocking 100% certification (Fail or Skipped).
    pub fn blocking_gates(&self) -> Vec<&'static str> {
        self.all_gates().iter().filter(|(_, res)| !res.is_pass()).map(|(name, _)| *name).collect()
    }

    /// Calculate exact pass percentage (0..100).
    pub fn pass_percentage(&self) -> f32 {
        let gates = self.all_gates();
        let passed = gates.iter().filter(|(_, r)| r.is_pass()).count();
        (passed as f32 / gates.len() as f32) * 100.0
    }
}

/// Classification of test corpus items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CorpusKind {
    SyntheticFixture,
    LegalOpenSourceRealWorld,
}

/// An individual test corpus item evaluated by Compatibility Lab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusItem {
    pub id: String,
    pub name: String,
    pub kind: CorpusKind,
    pub target_runtime: String,
    pub passed: bool,
}

/// Compatibility Lab test harness runner.
#[derive(Default)]
pub struct CompatibilityLab {
    corpus: Vec<CorpusItem>,
}

impl CompatibilityLab {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_item(&mut self, item: CorpusItem) {
        self.corpus.push(item);
    }

    pub fn run_all(&self) -> (usize, usize) {
        let passed = self.corpus.iter().filter(|i| i.passed).count();
        let total = self.corpus.len();
        (passed, total)
    }

    pub fn pass_rate_for_kind(&self, kind: CorpusKind) -> f32 {
        let items: Vec<_> = self.corpus.iter().filter(|i| i.kind == kind).collect();
        if items.is_empty() {
            return 0.0;
        }
        let passed = items.iter().filter(|i| i.passed).count();
        (passed as f32 / items.len() as f32) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fully_passing_matrix() -> CertifiedSupportMatrix {
        CertifiedSupportMatrix {
            matrix_id: "matrix-2026-pass".to_string(),
            runtime_target: "ald-0.1.0".to_string(),
            gta_legacy_build_2699: GateResult::Pass,
            gta_enhanced_build_3095: GateResult::Pass,
            cfxlua_profile: GateResult::Pass,
            native_js: GateResult::Pass,
            node16: GateResult::Pass,
            node22: GateResult::Pass,
            clr: GateResult::Pass,
            fxmanifest_supported_set: GateResult::Pass,
            citizen_required_corpus: GateResult::Pass,
            native_required_corpus: GateResult::Pass,
            asset_corpus: GateResult::Pass,
            datafile_corpus: GateResult::Pass,
            postgresql: GateResult::Pass,
            mysql: GateResult::Pass,
            mariadb: GateResult::Pass,
            esx_compat: GateResult::Pass,
            qbcore_compat: GateResult::Pass,
            qbox_compat: GateResult::Pass,
            streaming_chaos: GateResult::Pass,
            windows_build: GateResult::Pass,
            native_dependency_audit: GateResult::Pass,
        }
    }

    #[test]
    fn certification_passes_only_when_all_gates_pass() {
        let m = make_fully_passing_matrix();
        assert!(m.is_100_percent_certified());
        assert_eq!(m.pass_percentage(), 100.0);
        assert!(m.blocking_gates().is_empty());
    }

    #[test]
    fn certification_refuses_when_single_gate_fails() {
        let mut m = make_fully_passing_matrix();
        m.node22 = GateResult::Fail;

        assert!(!m.is_100_percent_certified());
        assert_eq!(m.blocking_gates(), vec!["node22"]);
        assert!(m.pass_percentage() < 100.0);
    }

    #[test]
    fn certification_refuses_when_gate_skipped() {
        let mut m = make_fully_passing_matrix();
        m.streaming_chaos = GateResult::Skipped;

        assert!(!m.is_100_percent_certified());
        assert_eq!(m.blocking_gates(), vec!["streaming_chaos"]);
    }

    #[test]
    fn blocking_gates_lists_multiple_failures() {
        let mut m = make_fully_passing_matrix();
        m.clr = GateResult::Fail;
        m.qbox_compat = GateResult::Skipped;

        let blockers = m.blocking_gates();
        assert!(blockers.contains(&"clr"));
        assert!(blockers.contains(&"qbox_compat"));
        assert_eq!(blockers.len(), 2);
    }

    #[test]
    fn corpus_registration_and_execution() {
        let mut lab = CompatibilityLab::new();
        lab.register_item(CorpusItem {
            id: "synth-1".to_string(),
            name: "event_ordering_wait0".to_string(),
            kind: CorpusKind::SyntheticFixture,
            target_runtime: "cfxlua".to_string(),
            passed: true,
        });
        lab.register_item(CorpusItem {
            id: "oss-1".to_string(),
            name: "esx_society_subset".to_string(),
            kind: CorpusKind::LegalOpenSourceRealWorld,
            target_runtime: "esx".to_string(),
            passed: true,
        });

        let (passed, total) = lab.run_all();
        assert_eq!(passed, 2);
        assert_eq!(total, 2);
    }

    #[test]
    fn synthetic_fixture_evaluation() {
        let mut lab = CompatibilityLab::new();
        lab.register_item(CorpusItem {
            id: "s-1".to_string(),
            name: "vec3_math".to_string(),
            kind: CorpusKind::SyntheticFixture,
            target_runtime: "cfxlua".to_string(),
            passed: true,
        });
        lab.register_item(CorpusItem {
            id: "s-2".to_string(),
            name: "joaat_hashes".to_string(),
            kind: CorpusKind::SyntheticFixture,
            target_runtime: "cfxlua".to_string(),
            passed: false,
        });

        assert_eq!(lab.pass_rate_for_kind(CorpusKind::SyntheticFixture), 50.0);
    }

    #[test]
    fn real_world_oss_corpus_evaluation() {
        let mut lab = CompatibilityLab::new();
        lab.register_item(CorpusItem {
            id: "oss-1".to_string(),
            name: "qb-inventory-readonly".to_string(),
            kind: CorpusKind::LegalOpenSourceRealWorld,
            target_runtime: "qbcore".to_string(),
            passed: true,
        });

        assert_eq!(lab.pass_rate_for_kind(CorpusKind::LegalOpenSourceRealWorld), 100.0);
    }

    #[test]
    fn matrix_serialization_roundtrip() {
        let m = make_fully_passing_matrix();
        let json = serde_json::to_string(&m).unwrap();
        let parsed: CertifiedSupportMatrix = serde_json::from_str(&json).unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn empty_lab_returns_zero_rate() {
        let lab = CompatibilityLab::new();
        assert_eq!(lab.pass_rate_for_kind(CorpusKind::SyntheticFixture), 0.0);
    }

    #[test]
    fn partial_matrix_exact_pass_percentage() {
        let mut m = make_fully_passing_matrix();
        m.mariadb = GateResult::Fail;
        // 20 out of 21 gates pass
        let expected = (20.0 / 21.0) * 100.0;
        let diff = (m.pass_percentage() - expected).abs();
        assert!(diff < 0.01);
    }
}
