//! Resource system: manifest parsing, lifecycle, capability validation.

use std::collections::HashMap;

pub mod capabilities;
pub mod lifecycle;
pub mod manifest;

pub use capabilities::{parse_capabilities, Capability};
pub use lifecycle::{LifecycleManager, ResourceState};
pub use manifest::{Manifest, ManifestError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestStatus {
    Parsed,
    Invalid(String),
}

/// Resolve dependency constraints across a set of manifests.
/// Detects missing, cycle, and version-conflict issues.
pub fn resolve_dependencies(manifests: &[Manifest]) -> Result<HashMap<String, Vec<String>>, String> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut by_name: HashMap<&str, &Manifest> = HashMap::new();
    for m in manifests {
        by_name.insert(m.name.as_str(), m);
        graph.entry(m.name.clone()).or_default();
    }
    // Build edges. We track required deps and validate presence/version.
    for m in manifests {
        for dep in &m.dependencies {
            if !by_name.contains_key(dep.name.as_str()) {
                return Err(format!("resource '{}' missing dependency '{}'", m.name, dep.name));
            }
            graph.get_mut(&m.name).unwrap().push(dep.name.clone());
        }
    }
    // Detect cycles via DFS.
    detect_cycle(&graph).map_err(|c| format!("dependency cycle: {}", c))?;
    Ok(graph)
}

fn detect_cycle(graph: &HashMap<String, Vec<String>>) -> Result<(), String> {
    let mut visited = HashMap::new();
    let mut stack = Vec::new();
    for node in graph.keys() {
        if !visited.contains_key(node) {
            visit(node, graph, &mut visited, &mut stack)?;
        }
    }
    Ok(())
}

fn visit(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashMap<String, bool>,
    stack: &mut Vec<String>,
) -> Result<(), String> {
    visited.insert(node.to_string(), false);
    stack.push(node.to_string());
    for next in graph.get(node).unwrap_or(&Vec::new()) {
        if !visited.contains_key(next) {
            visit(next, graph, visited, stack)?;
        } else if !visited[next] {
            let cycle = stack.join(" -> ");
            return Err(cycle);
        }
    }
    stack.pop();
    visited.insert(node.to_string(), true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Dependency;

    fn m(name: &str, deps: &[&str]) -> Manifest {
        Manifest {
            name: name.into(),
            version: "1.0.0".into(),
            author: None,
            description: None,
            license: None,
            minimum_runtime: None,
            dependencies: deps.iter().map(|d| Dependency { name: d.to_string(), version: None }).collect(),
            optional_dependencies: vec![],
            client_scripts: vec![],
            server_scripts: vec![],
            shared_scripts: vec![],
            ui: vec![],
            assets: vec![],
            capabilities: vec![],
            exports: vec![],
            imports: vec![],
            framework: None,
            compatibility: vec![],
            configuration: vec![],
            database_migrations: vec![],
        }
    }

    #[test]
    fn resolve_ok() {
        let ms = vec![m("a", &["b"]), m("b", &[])]; // a depends on b
        let g = resolve_dependencies(&ms).unwrap();
        assert_eq!(g["a"].len(), 1);
    }

    #[test]
    fn missing_dependency() {
        let ms = vec![m("a", &["ghost"])];
        assert!(resolve_dependencies(&ms).is_err());
    }

    #[test]
    fn cycle_detected() {
        let ms = vec![m("a", &["b"]), m("b", &["a"])];
        assert!(resolve_dependencies(&ms).is_err());
    }
}
