//! GTA DataFile registry: which meta files a resource declares, by category.
//!
//! A `data_file` manifest entry names a file the game must load as data
//! (handling, vehicle metadata, …). This registry answers: "is this filename
//! a recognized DataFile, and which category does it belong to?" so the
//! manifest layer, asset graph, and streaming scheduler share one table.
//!
//! Data honesty: only filenames stated in the platform spec's asset graphs
//! are seeded (the vehicle graph: `vehicles.meta`, `handling.meta`,
//! `carcols.meta`, `carvariations.meta`, `vehiclelayouts.meta`). Categories
//! for the remaining graphs exist as an enum, but their entries arrive with
//! their phases and Compatibility Lab evidence — not from memory. An entry
//! carries its source, and [`DataFileRegistry::unknown`] is a normal answer,
//! not an error.

use std::collections::BTreeMap;

use thiserror::Error;

/// DataFile category. Descriptive Aldivine labels for routing/graph work,
/// matching the platform spec's registry sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataFileCategory {
    Vehicle,
    Handling,
    Appearance,
    Variation,
    Layout,
    Ped,
    Clothing,
    MapInterior,
    Audio,
    Weapon,
    Clip,
    Population,
    Vfx,
    TextureParenting,
    Other,
}

/// One recognized DataFile: filename plus category and source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFileEntry {
    /// Lowercase filename, e.g. "vehicles.meta".
    pub file_name: String,
    pub category: DataFileCategory,
    pub source: EntrySource,
    pub note: String,
}

/// Where an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySource {
    /// Seeded from the platform spec's asset graphs. Routable; content
    /// behavior certified with its graph's phase.
    SpecSeed,
    /// Certified against real files with lab evidence (see note).
    Certified,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DataFileError {
    #[error("bad data file name '{0}'")]
    BadName(String),
    #[error("data file '{0}' already registered")]
    Duplicate(String),
}

/// The registry. Seed with [`DataFileRegistry::with_spec_seed`], extend via
/// [`DataFileRegistry::register`].
#[derive(Debug, Default)]
pub struct DataFileRegistry {
    entries: BTreeMap<String, DataFileEntry>,
}

impl DataFileRegistry {
    pub fn new() -> Self {
        DataFileRegistry::default()
    }

    /// Seed the spec's vehicle-graph filenames. Everything else arrives with
    /// its graph phase — this function must stay small on purpose.
    pub fn with_spec_seed() -> Self {
        let mut r = DataFileRegistry::new();
        let seed: &[(&str, DataFileCategory)] = &[
            ("vehicles.meta", DataFileCategory::Vehicle),
            ("handling.meta", DataFileCategory::Handling),
            ("carcols.meta", DataFileCategory::Appearance),
            ("carvariations.meta", DataFileCategory::Variation),
            ("vehiclelayouts.meta", DataFileCategory::Layout),
        ];
        for (name, category) in seed {
            r.entries.insert(
                name.to_string(),
                DataFileEntry {
                    file_name: name.to_string(),
                    category: *category,
                    source: EntrySource::SpecSeed,
                    note: "spec seed: vehicle graph".into(),
                },
            );
        }
        r
    }

    /// Register a DataFile. Names normalize to lowercase basename (manifests
    /// may carry paths); directories, streams, and garbage are rejected.
    /// Same upgrade rule as the format registry: `SpecSeed` → `Certified`
    /// may replace, anything else duplicates.
    pub fn register(
        &mut self,
        name: &str,
        category: DataFileCategory,
        source: EntrySource,
        note: &str,
    ) -> Result<(), DataFileError> {
        let key = normalize_name(name)?;
        match self.entries.get(&key) {
            Some(existing) if existing.source == EntrySource::SpecSeed && source == EntrySource::Certified => {}
            Some(_) => return Err(DataFileError::Duplicate(key)),
            None => {}
        }
        self.entries.insert(key.clone(), DataFileEntry { file_name: key, category, source, note: note.to_string() });
        Ok(())
    }

    /// Look up by filename or manifest path. Unknown names return `None` —
    /// callers decide whether unknown is an error (strict manifests) or a
    /// plain streamed file.
    pub fn lookup(&self, name_or_path: &str) -> Option<&DataFileEntry> {
        let key = normalize_name(name_or_path).ok()?;
        self.entries.get(&key)
    }

    /// All entries of one category, sorted by filename.
    pub fn by_category(&self, category: DataFileCategory) -> Vec<&DataFileEntry> {
        self.entries.values().filter(|e| e.category == category).collect()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

fn normalize_name(raw: &str) -> Result<String, DataFileError> {
    let t = raw.trim();
    if t.is_empty() || t.contains('\0') {
        return Err(DataFileError::BadName(raw.to_string()));
    }
    // Colons are rejected (NTFS alternate data streams), except a Windows
    // drive prefix ("C:\…"), which manifests and scanners legitimately carry.
    let without_drive = match t.as_bytes() {
        [drive, b':', sep, ..] if drive.is_ascii_alphabetic() && (*sep == b'\\' || *sep == b'/') => &t[3..],
        _ if t.contains(':') => return Err(DataFileError::BadName(raw.to_string())),
        _ => t,
    };
    let base = without_drive.rsplit(['/', '\\']).next().unwrap_or(without_drive);
    if base.is_empty() || base == "." || base == ".." || base.len() > 128 {
        return Err(DataFileError::BadName(raw.to_string()));
    }
    if !base.contains('.') {
        return Err(DataFileError::BadName(raw.to_string()));
    }
    Ok(base.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_seed_is_vehicle_graph_only() {
        let r = DataFileRegistry::with_spec_seed();
        assert_eq!(r.entry_count(), 5);
        for name in ["vehicles.meta", "handling.meta", "carcols.meta", "carvariations.meta", "vehiclelayouts.meta"] {
            let e = r.lookup(name).unwrap_or_else(|| panic!("{name} seeded"));
            assert_eq!(e.source, EntrySource::SpecSeed);
        }
        // Deliberately NOT seeded: other graphs arrive with their phases.
        assert_eq!(r.lookup("pedmetadata.meta"), None);
    }

    #[test]
    fn lookup_normalizes_paths_and_case() {
        let r = DataFileRegistry::with_spec_seed();
        assert_eq!(r.lookup("VEHICLES.META").map(|e| e.category), Some(DataFileCategory::Vehicle));
        assert_eq!(
            r.lookup("dlcpacks/patchday1ng/dlc.rpf/common/data/handling.meta").map(|e| e.category),
            Some(DataFileCategory::Handling)
        );
        assert_eq!(r.lookup("C:\\mods\\CARCOLS.META").map(|e| e.category), Some(DataFileCategory::Appearance));
    }

    #[test]
    fn unknown_is_none_not_error() {
        let r = DataFileRegistry::with_spec_seed();
        for name in ["handling2.meta", "readme.txt", "", "noextension", ".", "..", "a:b.meta"] {
            assert_eq!(r.lookup(name), None, "{name:?}");
        }
    }

    #[test]
    fn register_and_upgrade_rules() {
        let mut r = DataFileRegistry::with_spec_seed();
        r.register("pedmetadata.meta", DataFileCategory::Ped, EntrySource::SpecSeed, "graph phase").unwrap();
        assert_eq!(r.lookup("PEDMETADATA.META").map(|e| e.category), Some(DataFileCategory::Ped));
        // Duplicate at same level refused.
        assert_eq!(
            r.register("pedmetadata.meta", DataFileCategory::Ped, EntrySource::SpecSeed, "x"),
            Err(DataFileError::Duplicate("pedmetadata.meta".into()))
        );
        // Seed -> certified upgrade allowed, and sticks.
        r.register("vehicles.meta", DataFileCategory::Vehicle, EntrySource::Certified, "lab run #1").unwrap();
        assert_eq!(r.lookup("vehicles.meta").map(|e| e.source), Some(EntrySource::Certified));
        assert_eq!(
            r.register("vehicles.meta", DataFileCategory::Vehicle, EntrySource::Certified, "x"),
            Err(DataFileError::Duplicate("vehicles.meta".into()))
        );
    }

    #[test]
    fn bad_names_rejected() {
        let mut r = DataFileRegistry::new();
        for bad in ["", "   ", "noextension", ".", "..", "a:b.meta", "x.meta:stream", &"m".repeat(130)] {
            assert!(r.register(bad, DataFileCategory::Other, EntrySource::SpecSeed, "").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn by_category_lists_sorted() {
        let mut r = DataFileRegistry::with_spec_seed();
        r.register("b_handling.meta", DataFileCategory::Handling, EntrySource::SpecSeed, "").unwrap();
        r.register("a_handling.meta", DataFileCategory::Handling, EntrySource::SpecSeed, "").unwrap();
        let names: Vec<&str> = r.by_category(DataFileCategory::Handling).iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, vec!["a_handling.meta", "b_handling.meta", "handling.meta"]);
        assert!(r.by_category(DataFileCategory::Audio).is_empty());
    }
}
