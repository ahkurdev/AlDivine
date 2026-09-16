//! GTA asset format registry: recognition, not parsing.
//!
//! Answers one question: "this file claims to be X — is X a format Aldivine
//! recognizes, and how should the streaming pipeline treat it?" Nothing here
//! opens, decodes, or validates file contents; content verification arrives
//! with the streaming/cache phases against real files.
//!
//! [`StreamClass`] is Aldivine's routing label (which scheduler lane, which
//! residency path), not a claim about Rockstar internals. The extension
//! table seeds from the platform spec's format list; entries carry their
//! source (`SpecSeed` vs `Certified`) so certified-vs-assumed never blurs.

use std::collections::BTreeMap;

use thiserror::Error;

/// How Aldivine routes a recognized file through streaming/mount/residency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamClass {
    /// Texture dictionaries and standalone textures.
    Texture,
    /// Geometry: drawables, fragments, draw dictionaries.
    Geometry,
    /// Collision bounds.
    Collision,
    /// Map placement and type definitions.
    Map,
    /// Clip dictionaries (animation).
    Animation,
    /// Audio containers and metadata.
    Audio,
    /// Data/meta files (vehicles.meta, handling.meta, …).
    Metadata,
    /// Navigation and miscellaneous game data.
    Data,
}

/// Where a registry entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySource {
    /// Seeded from the platform spec's format list. Recognized for routing;
    /// content behavior uncertified until Compatibility Lab evidence.
    SpecSeed,
    /// Certified against real files with lab evidence (see note).
    Certified,
}

/// One recognized format: extension (with dot, lowercase) plus routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetFormat {
    /// e.g. ".ytd". Always lowercase with leading dot.
    pub extension: String,
    pub class: StreamClass,
    pub source: EntrySource,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryError {
    #[error("bad extension '{0}' (expected like \".ytd\")")]
    BadExtension(String),
    #[error("extension '{0}' already registered")]
    Duplicate(String),
}

/// Recognition result for a file path or bare extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognition {
    Known(AssetFormat),
    Unknown { extension: String },
}

/// The format registry. Built with [`FormatRegistry::with_spec_seed`] plus
/// operator/lab additions via [`FormatRegistry::register`].
#[derive(Debug, Default)]
pub struct FormatRegistry {
    formats: BTreeMap<String, AssetFormat>,
}

impl FormatRegistry {
    pub fn new() -> Self {
        FormatRegistry::default()
    }

    /// Seed the spec's format list. Every entry is `SpecSeed`: routable,
    /// content-uncertified. See `docs/compatibility/ASSET_FORMAT_MATRIX.md`.
    pub fn with_spec_seed() -> Self {
        let mut r = FormatRegistry::new();
        // (extension, class) — the extension list is the platform spec's;
        // classes are Aldivine routing labels.
        let seed: &[(&str, StreamClass)] = &[
            (".ytd", StreamClass::Texture),
            (".yft", StreamClass::Geometry),
            (".ydd", StreamClass::Geometry),
            (".ydr", StreamClass::Geometry),
            (".ybn", StreamClass::Collision),
            (".ymap", StreamClass::Map),
            (".ytyp", StreamClass::Map),
            (".ycd", StreamClass::Animation),
            (".awc", StreamClass::Audio),
            (".rel", StreamClass::Data),
            (".ymt", StreamClass::Metadata),
        ];
        for (ext, class) in seed {
            r.formats.insert(
                ext.to_string(),
                AssetFormat {
                    extension: ext.to_string(),
                    class: *class,
                    source: EntrySource::SpecSeed,
                    note: "spec seed: recognized for routing, content uncertified".into(),
                },
            );
        }
        r
    }

    /// Register (or override with a certified entry) a format. Extension is
    /// normalized (trimmed, lowercased, dot-prefixed); garbage rejected.
    /// Re-registering an identical extension is a [`RegistryError::Duplicate`]
    /// unless it upgrades `SpecSeed` → `Certified`.
    pub fn register(
        &mut self,
        extension: &str,
        class: StreamClass,
        source: EntrySource,
        note: &str,
    ) -> Result<(), RegistryError> {
        let ext = normalize_extension(extension)?;
        match self.formats.get(&ext) {
            Some(existing) if existing.source == EntrySource::SpecSeed && source == EntrySource::Certified => {}
            Some(_) => return Err(RegistryError::Duplicate(ext)),
            None => {}
        }
        self.formats.insert(ext.clone(), AssetFormat { extension: ext, class, source, note: note.to_string() });
        Ok(())
    }

    /// Recognize by file path or bare extension. Extension match is
    /// case-insensitive; paths without a usable extension are `Unknown`
    /// with an empty extension, never an error (recognition is total).
    pub fn recognize(&self, path_or_ext: &str) -> Recognition {
        match extension_of(path_or_ext) {
            Some(ext) => match self.formats.get(&ext) {
                Some(f) => Recognition::Known(f.clone()),
                None => Recognition::Unknown { extension: ext },
            },
            None => Recognition::Unknown { extension: String::new() },
        }
    }

    pub fn format_count(&self) -> usize {
        self.formats.len()
    }

    /// All registered formats, sorted by extension.
    pub fn all(&self) -> Vec<&AssetFormat> {
        self.formats.values().collect()
    }
}

fn normalize_extension(raw: &str) -> Result<String, RegistryError> {
    let t = raw.trim().to_ascii_lowercase();
    if t.contains('\0') || t.contains('/') || t.contains('\\') || t.contains(':') {
        return Err(RegistryError::BadExtension(raw.to_string()));
    }
    let ext = match extension_of(&t) {
        Some(e) => e,
        None => {
            // Bare word without dots ("ytd") normalizes to ".ytd".
            if t.is_empty() || t.len() > 16 || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(RegistryError::BadExtension(raw.to_string()));
            }
            format!(".{t}")
        }
    };
    if ext.len() > 17 {
        return Err(RegistryError::BadExtension(raw.to_string()));
    }
    Ok(ext)
}

/// Lowercase extension with dot from a path or extension string. Returns
/// `None` when no usable extension exists (no dot, trailing dot, spaces).
fn extension_of(s: &str) -> Option<String> {
    let base = s.trim().rsplit(['/', '\\']).next().unwrap_or(s);
    let (_, ext) = base.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 16 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(format!(".{}", ext.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_seed_covers_spec_list() {
        let r = FormatRegistry::with_spec_seed();
        assert_eq!(r.format_count(), 11);
        for ext in [".ytd", ".yft", ".ydd", ".ydr", ".ybn", ".ymap", ".ytyp", ".ycd", ".awc", ".rel", ".ymt"] {
            match r.recognize(ext) {
                Recognition::Known(f) => {
                    assert_eq!(f.source, EntrySource::SpecSeed);
                    assert_eq!(f.extension, ext);
                }
                Recognition::Unknown { .. } => panic!("{ext} should be seeded"),
            }
        }
    }

    #[test]
    fn recognition_routes_classes() {
        let r = FormatRegistry::with_spec_seed();
        let class_of = |p: &str| match r.recognize(p) {
            Recognition::Known(f) => f.class,
            Recognition::Unknown { .. } => panic!("{p} should be known"),
        };
        assert_eq!(class_of("vehiclemodels.yft"), StreamClass::Geometry);
        assert_eq!(class_of("C:\\mods\\tex.YTD"), StreamClass::Texture);
        assert_eq!(class_of("dlc.rpf\\x64\\levels\\gta5\\citye.map\\2k_highway.ymap"), StreamClass::Map);
        assert_eq!(class_of(" Clip.YCD "), StreamClass::Animation);
    }

    #[test]
    fn unknown_stays_unknown() {
        let r = FormatRegistry::with_spec_seed();
        for p in ["readme.txt", "GTA5.exe", ".dll", "noextension", "", ".", "archive.tar.gz", "weird.ytdx"] {
            assert!(matches!(r.recognize(p), Recognition::Unknown { .. }), "{p:?}");
        }
        // Multi-dot: only the final extension counts.
        assert!(matches!(r.recognize("backup.ytd.bak"), Recognition::Unknown { .. }));
    }

    #[test]
    fn register_certified_and_duplicates() {
        let mut r = FormatRegistry::with_spec_seed();
        // Upgrade seed -> certified: allowed.
        r.register(".ytd", StreamClass::Texture, EntrySource::Certified, "lab run #1").unwrap();
        match r.recognize(".ytd") {
            Recognition::Known(f) => {
                assert_eq!(f.source, EntrySource::Certified);
                assert_eq!(f.note, "lab run #1");
            }
            _ => panic!("should stay known"),
        }
        // Same-level re-register: refused.
        assert_eq!(
            r.register(".ytd", StreamClass::Texture, EntrySource::Certified, "x"),
            Err(RegistryError::Duplicate(".ytd".into()))
        );
        // New format, bare word normalizes.
        r.register("ydd2", StreamClass::Geometry, EntrySource::SpecSeed, "test").unwrap();
        assert!(matches!(r.recognize("a.YDD2"), Recognition::Known(_)));
    }

    #[test]
    fn bad_registrations_rejected() {
        let mut r = FormatRegistry::new();
        for bad in [
            "",
            "   ",
            ".",
            "..",
            ".ytd ",
            "a/b.ytd",
            "c:\\x.ytd",
            "d:ytd",
            "waytoolongextensionname",
            ".ytd:stream",
            "y td",
        ] {
            // ".ytd " trims to a valid ".ytd": handle separately below.
            if bad == ".ytd " {
                continue;
            }
            assert!(r.register(bad, StreamClass::Data, EntrySource::SpecSeed, "").is_err(), "{bad:?}");
        }
        // Whitespace-padded valid extension registers trimmed.
        r.register(".ytd ", StreamClass::Texture, EntrySource::SpecSeed, "").unwrap();
        assert!(matches!(r.recognize("x.ytd"), Recognition::Known(_)));
    }

    #[test]
    fn all_sorted_by_extension() {
        let r = FormatRegistry::with_spec_seed();
        let exts: Vec<&str> = r.all().iter().map(|f| f.extension.as_str()).collect();
        let mut sorted = exts.clone();
        sorted.sort_unstable();
        assert_eq!(exts, sorted);
    }
}
