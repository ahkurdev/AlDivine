//! Aldivine Upgrade Compatibility Planner
//!
//! Analyzes compatibility graphs between the running environment and target release:
//! - NovaGate & Astryn client versions
//! - Server Runtime & AstraNet protocol versions
//! - Aldivine Framework & Aegis control versions
//! - Pinned package versions & major breaking changes
//! - Relational database schema migration requirements
//! - Rollback boundaries & restart severity classification

use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// An environment topology graph defining exact component versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeGraph {
    pub novagate: String,
    pub astryn: String,
    pub server_runtime: String,
    pub astranet_protocol: u32,
    pub framework: String,
    pub aegis: String,
    pub db_version: i64,
    pub packages: HashMap<String, String>,
}

impl Default for UpgradeGraph {
    fn default() -> Self {
        Self {
            novagate: "0.1.0".to_string(),
            astryn: "0.1.0".to_string(),
            server_runtime: "0.1.0".to_string(),
            astranet_protocol: 1,
            framework: "0.1.0".to_string(),
            aegis: "0.1.0".to_string(),
            db_version: 1,
            packages: HashMap::new(),
        }
    }
}

/// A detected incompatibility between current and target versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incompatibility {
    pub component: String,
    pub current: String,
    pub target: String,
    pub reason: String,
    pub is_breaking: bool,
}

/// Client upgrade requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientUpdateRequirement {
    pub client: String,
    pub from_version: String,
    pub to_version: String,
    pub mandatory: bool,
}

/// Database schema migration requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbMigrationRequirement {
    pub from_version: i64,
    pub to_version: i64,
    pub steps: i64,
    pub destructive: bool,
}

/// Permissible rollback boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackLimit {
    Safe,
    Conditional { warning: String },
    Impossible { reason: String },
}

/// Level of server restart required to apply the upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RestartRequirement {
    None = 0,
    HotReload = 1,
    GracefulServerRestart = 2,
    FullStackRestart = 3,
}

/// Comprehensive plan produced by the upgrade planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradePlan {
    pub current: UpgradeGraph,
    pub target: UpgradeGraph,
    pub incompatibilities: Vec<Incompatibility>,
    pub client_updates: Vec<ClientUpdateRequirement>,
    pub db_migration: Option<DbMigrationRequirement>,
    pub rollback_limit: RollbackLimit,
    pub restart_requirement: RestartRequirement,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpgradePlannerError {
    #[error("invalid semver for {component} '{version}': {details}")]
    InvalidSemver { component: String, version: String, details: String },
}

/// Analyzes current vs target upgrade topologies.
pub struct UpgradePlanner;

impl UpgradePlanner {
    pub fn plan(current: &UpgradeGraph, target: &UpgradeGraph) -> Result<UpgradePlan, UpgradePlannerError> {
        let mut incompatibilities = Vec::new();
        let mut client_updates = Vec::new();
        let mut restart_requirement = RestartRequirement::None;

        // 1. Protocol compatibility
        if target.astranet_protocol != current.astranet_protocol {
            let is_breaking = target.astranet_protocol > current.astranet_protocol;
            incompatibilities.push(Incompatibility {
                component: "AstraNet".to_string(),
                current: current.astranet_protocol.to_string(),
                target: target.astranet_protocol.to_string(),
                reason: if is_breaking {
                    "Protocol version bumped; older clients cannot connect".to_string()
                } else {
                    "Target protocol version is older than current".to_string()
                },
                is_breaking,
            });
            restart_requirement = restart_requirement.max(RestartRequirement::GracefulServerRestart);
        }

        // 2. Server runtime version
        let cur_srv = parse_version("server_runtime", &current.server_runtime)?;
        let tgt_srv = parse_version("server_runtime", &target.server_runtime)?;
        if cur_srv != tgt_srv {
            restart_requirement = restart_requirement.max(RestartRequirement::GracefulServerRestart);
            if tgt_srv.major > cur_srv.major {
                incompatibilities.push(Incompatibility {
                    component: "server_runtime".to_string(),
                    current: current.server_runtime.clone(),
                    target: target.server_runtime.clone(),
                    reason: "Major server runtime bump".to_string(),
                    is_breaking: true,
                });
            }
        }

        // 3. Astryn client version & protocol requirement
        let cur_astryn = parse_version("astryn", &current.astryn)?;
        let tgt_astryn = parse_version("astryn", &target.astryn)?;
        let protocol_bumped = target.astranet_protocol > current.astranet_protocol;
        if cur_astryn != tgt_astryn || protocol_bumped {
            let mandatory = tgt_astryn.major > cur_astryn.major || protocol_bumped;
            client_updates.push(ClientUpdateRequirement {
                client: "astryn".to_string(),
                from_version: current.astryn.clone(),
                to_version: target.astryn.clone(),
                mandatory,
            });
        }

        // 4. Framework version
        let cur_fw = parse_version("framework", &current.framework)?;
        let tgt_fw = parse_version("framework", &target.framework)?;
        if tgt_fw.major > cur_fw.major {
            incompatibilities.push(Incompatibility {
                component: "framework".to_string(),
                current: current.framework.clone(),
                target: target.framework.clone(),
                reason: "Major framework version upgrade".to_string(),
                is_breaking: true,
            });
            restart_requirement = restart_requirement.max(RestartRequirement::GracefulServerRestart);
        }

        // 5. Packages semver checks
        let mut packages_changed = false;
        for (pkg_name, tgt_ver_str) in &target.packages {
            if let Some(cur_ver_str) = current.packages.get(pkg_name) {
                let cur_ver = parse_version(pkg_name, cur_ver_str)?;
                let tgt_ver = parse_version(pkg_name, tgt_ver_str)?;
                if cur_ver != tgt_ver {
                    packages_changed = true;
                    if tgt_ver.major > cur_ver.major || (cur_ver.major == 0 && tgt_ver.minor > cur_ver.minor) {
                        incompatibilities.push(Incompatibility {
                            component: format!("package:{}", pkg_name),
                            current: cur_ver_str.clone(),
                            target: tgt_ver_str.clone(),
                            reason: "Breaking package semver upgrade".to_string(),
                            is_breaking: true,
                        });
                    }
                }
            } else {
                packages_changed = true;
            }
        }
        if packages_changed {
            restart_requirement = restart_requirement.max(RestartRequirement::HotReload);
        }

        // 6. DB schema migration & rollback limits
        let db_migration = if target.db_version != current.db_version {
            let steps = (target.db_version - current.db_version).abs();
            let destructive = target.db_version < current.db_version;
            restart_requirement = restart_requirement.max(RestartRequirement::FullStackRestart);
            Some(DbMigrationRequirement {
                from_version: current.db_version,
                to_version: target.db_version,
                steps,
                destructive,
            })
        } else {
            None
        };

        let rollback_limit = if let Some(ref db) = db_migration {
            if db.destructive {
                RollbackLimit::Impossible {
                    reason: format!(
                        "Target DB schema version ({}) is older than current ({}); non-reversible migration",
                        target.db_version, current.db_version
                    ),
                }
            } else {
                RollbackLimit::Conditional {
                    warning: "Rollback requires explicit database migration rollback script".to_string(),
                }
            }
        } else if incompatibilities.iter().any(|i| i.is_breaking) {
            RollbackLimit::Conditional {
                warning: "Breaking major changes detected in components or packages".to_string(),
            }
        } else {
            RollbackLimit::Safe
        };

        Ok(UpgradePlan {
            current: current.clone(),
            target: target.clone(),
            incompatibilities,
            client_updates,
            db_migration,
            rollback_limit,
            restart_requirement,
        })
    }
}

fn parse_version(component: &str, s: &str) -> Result<Version, UpgradePlannerError> {
    Version::parse(s).map_err(|e| UpgradePlannerError::InvalidSemver {
        component: component.to_string(),
        version: s.to_string(),
        details: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_graphs_require_no_restart_or_migration() {
        let cur = UpgradeGraph::default();
        let plan = UpgradePlanner::plan(&cur, &cur).unwrap();

        assert_eq!(plan.restart_requirement, RestartRequirement::None);
        assert!(plan.incompatibilities.is_empty());
        assert!(plan.client_updates.is_empty());
        assert_eq!(plan.db_migration, None);
        assert_eq!(plan.rollback_limit, RollbackLimit::Safe);
    }

    #[test]
    fn package_update_only_requires_hot_reload() {
        let cur = UpgradeGraph::default();
        let mut tgt = cur.clone();
        tgt.packages.insert("chat".to_string(), "0.1.1".to_string());

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert_eq!(plan.restart_requirement, RestartRequirement::HotReload);
        assert!(plan.incompatibilities.is_empty());
    }

    #[test]
    fn server_runtime_patch_requires_graceful_restart() {
        let cur = UpgradeGraph::default();
        let mut tgt = cur.clone();
        tgt.server_runtime = "0.1.1".to_string();

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert_eq!(plan.restart_requirement, RestartRequirement::GracefulServerRestart);
        assert_eq!(plan.rollback_limit, RollbackLimit::Safe);
    }

    #[test]
    fn protocol_bump_flags_incompatibility_and_client_update() {
        let cur = UpgradeGraph::default();
        let mut tgt = cur.clone();
        tgt.astranet_protocol = 2;

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert!(plan.incompatibilities.iter().any(|i| i.component == "AstraNet" && i.is_breaking));
        assert!(plan.client_updates.iter().any(|c| c.client == "astryn" && c.mandatory));
        assert_eq!(plan.restart_requirement, RestartRequirement::GracefulServerRestart);
    }

    #[test]
    fn forward_db_migration_triggers_full_stack_restart() {
        let cur = UpgradeGraph::default();
        let mut tgt = cur.clone();
        tgt.db_version = 4;

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert_eq!(plan.restart_requirement, RestartRequirement::FullStackRestart);
        let db = plan.db_migration.unwrap();
        assert_eq!(db.from_version, 1);
        assert_eq!(db.to_version, 4);
        assert_eq!(db.steps, 3);
        assert!(!db.destructive);
        assert!(matches!(plan.rollback_limit, RollbackLimit::Conditional { .. }));
    }

    #[test]
    fn backward_db_migration_flags_rollback_impossible() {
        let cur = UpgradeGraph { db_version: 5, ..Default::default() };
        let mut tgt = cur.clone();
        tgt.db_version = 3;

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert!(matches!(plan.rollback_limit, RollbackLimit::Impossible { .. }));
        assert!(plan.db_migration.unwrap().destructive);
    }

    #[test]
    fn breaking_package_major_bump_flagged() {
        let mut cur = UpgradeGraph::default();
        cur.packages.insert("economy".to_string(), "1.0.0".to_string());
        let mut tgt = cur.clone();
        tgt.packages.insert("economy".to_string(), "2.0.0".to_string());

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert!(plan.incompatibilities.iter().any(|i| i.component == "package:economy" && i.is_breaking));
    }

    #[test]
    fn framework_major_update_flags_breaking() {
        let cur = UpgradeGraph::default();
        let mut tgt = cur.clone();
        tgt.framework = "2.0.0".to_string();

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert!(plan.incompatibilities.iter().any(|i| i.component == "framework" && i.is_breaking));
    }

    #[test]
    fn astryn_major_update_mandates_client_update() {
        let cur = UpgradeGraph::default();
        let mut tgt = cur.clone();
        tgt.astryn = "1.0.0".to_string();

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        let client_up = plan.client_updates.iter().find(|c| c.client == "astryn").unwrap();
        assert!(client_up.mandatory);
    }

    #[test]
    fn complex_multi_component_plan_generation() {
        let mut cur = UpgradeGraph::default();
        cur.packages.insert("core".to_string(), "0.1.0".to_string());

        let mut tgt = cur.clone();
        tgt.server_runtime = "0.2.0".to_string();
        tgt.astranet_protocol = 2;
        tgt.db_version = 3;
        tgt.packages.insert("core".to_string(), "0.2.0".to_string());

        let plan = UpgradePlanner::plan(&cur, &tgt).unwrap();
        assert_eq!(plan.restart_requirement, RestartRequirement::FullStackRestart);
        assert!(plan.db_migration.is_some());
        assert!(!plan.incompatibilities.is_empty());
    }
}
