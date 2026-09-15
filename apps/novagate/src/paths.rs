//! Aldivine local data layout.
//!
//! Aldivine stores everything under %LOCALAPPDATA%/Aldivine and never writes
//! into the base GTA V installation.

use std::path::PathBuf;

/// All NovaGate/Astryn local state, isolated from the game install.
#[derive(Debug, Clone)]
pub struct AldivinePaths {
    root: PathBuf,
}

impl AldivinePaths {
    /// Resolve from the platform convention. Falls back to a temp dir if
    /// LOCALAPPDATA is unavailable (tests, containers).
    pub fn resolve() -> Self {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir())
            .join("Aldivine");
        AldivinePaths::from_root(root)
    }

    pub fn from_root(root: PathBuf) -> Self {
        let p = AldivinePaths { root };
        p.ensure();
        p
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
    pub fn runtime(&self) -> PathBuf {
        self.root.join("Runtime")
    }
    pub fn cache(&self) -> PathBuf {
        self.root.join("Cache")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("Logs")
    }
    pub fn resources(&self) -> PathBuf {
        self.root.join("Resources")
    }
    pub fn versions(&self) -> PathBuf {
        self.root.join("Versions")
    }
    pub fn crash_reports(&self) -> PathBuf {
        self.root.join("CrashReports")
    }

    /// Create the whole tree. Best-effort; callers that need a specific dir
    /// should create it explicitly before writing.
    fn ensure(&self) {
        for d in [self.runtime(), self.cache(), self.logs(), self.resources(), self.versions(), self.crash_reports()] {
            let _ = std::fs::create_dir_all(&d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_game_install_independent() {
        let p = AldivinePaths::from_root(std::env::temp_dir().join("ald-paths-test"));
        // Every path descends from the Aldivine root, never from a GTA dir.
        for d in [p.runtime(), p.cache(), p.logs(), p.resources(), p.versions(), p.crash_reports()] {
            assert!(d.starts_with(p.root()));
        }
    }

    #[test]
    fn directories_created() {
        let root = std::env::temp_dir().join(format!("ald-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = AldivinePaths::from_root(root.clone());
        assert!(p.logs().is_dir());
        assert!(p.crash_reports().is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_uses_localappdata_or_temp() {
        // Must not panic when LOCALAPPDATA is unset.
        let _ = AldivinePaths::resolve();
    }
}
