//! The corpus manifest (`tests/corpus/manifest.toml`): one [`Case`] per dataset.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::OracleError;
use crate::record::{OracleRecord, SCHEMA};

/// Manifest schema version understood by this crate.
pub const MANIFEST_SCHEMA: u32 = 1;

/// Format directory names currently in the corpus. Adding a Tier 1 format = add its name here,
/// a reader function in `golden.py`, builders in `generate.py`, and cases in the manifest.
pub const KNOWN_FORMATS: &[&str] = &["shapefile", "geopackage", "geotiff"];

/// Parsed `manifest.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest schema version ([`MANIFEST_SCHEMA`]).
    pub schema: u32,
    /// All cases, in manifest order.
    #[serde(rename = "case")]
    pub cases: Vec<Case>,
}

/// Where a case's data comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Built by `tests/corpus/scripts/generate.py` (regenerated on every run).
    Synthetic,
    /// Third-party data, committed or fetched as-is with recorded provenance and licence.
    RealWorld,
}

/// Where a case's files live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Location {
    /// `tests/corpus/data/<format>/<id>/` — committed.
    #[default]
    Committed,
    /// `tests/corpus/external/<format>/<id>/` — fetched by `scripts/fetch-external.*`, gitignored.
    External,
}

/// One corpus case: a single dataset (one anchor file plus its members).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// Unique id; also the case directory name and the golden file stem.
    pub id: String,
    /// Format directory name (one of [`KNOWN_FORMATS`]); adapters are registered per format.
    pub format: String,
    /// Synthetic or real-world.
    pub origin: Origin,
    /// Committed or external.
    #[serde(default)]
    pub location: Location,
    /// The dataset's anchor file (e.g. `roads.shp`), one of `files`.
    pub anchor: String,
    /// Every file in the case directory (plain file names, sorted).
    pub files: Vec<String>,
    /// Edge-case tags (vocabulary in `tests/corpus/README.md`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// One-line description of what the case exercises.
    pub description: String,
    /// Where the data came from (generator script, or upstream URL at a pinned revision).
    pub provenance: String,
    /// Licence of the data (SPDX expression or a short statement + URL).
    pub licence: String,
    /// Optional upstream sha256 per file (real-world data), for re-verification against the source.
    #[serde(default)]
    pub sha256: BTreeMap<String, String>,
}

impl Case {
    /// Whether the case carries `tag`.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Deliberately corrupt/truncated input: native readers must return `Error`/`Partial`.
    pub fn is_corrupt(&self) -> bool {
        self.has_tag("corrupt")
    }
}

/// A loaded, validated corpus rooted at `tests/corpus/`.
#[derive(Debug, Clone)]
pub struct Corpus {
    root: PathBuf,
    manifest: Manifest,
}

impl Corpus {
    /// Load and validate `<root>/manifest.toml`.
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, OracleError> {
        let root = root.into();
        let path = root.join("manifest.toml");
        let text = read_to_string(&path)?;
        let manifest: Manifest =
            toml::from_str(&text).map_err(|source| OracleError::Toml { path, source })?;
        let corpus = Self { root, manifest };
        corpus.validate()?;
        Ok(corpus)
    }

    /// `tests/corpus/`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All cases in manifest order.
    pub fn cases(&self) -> &[Case] {
        &self.manifest.cases
    }

    /// Look a case up by id.
    pub fn case(&self, id: &str) -> Option<&Case> {
        self.manifest.cases.iter().find(|c| c.id == id)
    }

    /// Directory holding the case's files.
    pub fn case_dir(&self, case: &Case) -> PathBuf {
        let base = match case.location {
            Location::Committed => "data",
            Location::External => "external",
        };
        self.root.join(base).join(&case.format).join(&case.id)
    }

    /// Whether every file of the case exists (external cases may not have been fetched).
    pub fn is_present(&self, case: &Case) -> bool {
        let dir = self.case_dir(case);
        case.files.iter().all(|f| dir.join(f).is_file())
    }

    /// `tests/corpus/expected/<id>.json`.
    pub fn expected_path(&self, case: &Case) -> PathBuf {
        self.root.join("expected").join(format!("{}.json", case.id))
    }

    /// Load and sanity-check the case's golden oracle record.
    pub fn load_expected(&self, case: &Case) -> Result<OracleRecord, OracleError> {
        let path = self.expected_path(case);
        let text = read_to_string(&path)?;
        let record: OracleRecord =
            serde_json::from_str(&text).map_err(|source| OracleError::Json {
                path: path.clone(),
                source,
            })?;
        if record.schema != SCHEMA {
            return Err(invalid(format!(
                "{}: record schema {} (this crate understands {SCHEMA})",
                path.display(),
                record.schema
            )));
        }
        if record.case != case.id || record.format != case.format {
            return Err(invalid(format!(
                "{}: record is for {}/{}, manifest says {}/{}",
                path.display(),
                record.format,
                record.case,
                case.format,
                case.id
            )));
        }
        Ok(record)
    }

    fn validate(&self) -> Result<(), OracleError> {
        if self.manifest.schema != MANIFEST_SCHEMA {
            return Err(invalid(format!(
                "manifest schema {} (this crate understands {MANIFEST_SCHEMA})",
                self.manifest.schema
            )));
        }
        let mut ids = BTreeSet::new();
        for case in &self.manifest.cases {
            let id = &case.id;
            if !is_plain_name(id) {
                return Err(invalid(format!("case id {id:?} is not a plain name")));
            }
            if !ids.insert(id.as_str()) {
                return Err(invalid(format!("duplicate case id {id:?}")));
            }
            if !KNOWN_FORMATS.contains(&case.format.as_str()) {
                return Err(invalid(format!("{id}: unknown format {:?}", case.format)));
            }
            if case.files.is_empty() {
                return Err(invalid(format!("{id}: no files listed")));
            }
            if let Some(bad) = case.files.iter().find(|f| !is_plain_name(f)) {
                return Err(invalid(format!(
                    "{id}: file {bad:?} is not a plain file name"
                )));
            }
            if !case.files.contains(&case.anchor) {
                return Err(invalid(format!(
                    "{id}: anchor {:?} not in files",
                    case.anchor
                )));
            }
            if let Some(extra) = case.sha256.keys().find(|k| !case.files.contains(k)) {
                return Err(invalid(format!("{id}: sha256 for unlisted file {extra:?}")));
            }
            for (field, value) in [
                ("description", &case.description),
                ("provenance", &case.provenance),
                ("licence", &case.licence),
            ] {
                if value.trim().is_empty() {
                    return Err(invalid(format!("{id}: empty {field}")));
                }
            }
        }
        Ok(())
    }
}

/// A single path component with no separators, no `..`, nothing hidden: the manifest must not be
/// able to point outside the corpus.
fn is_plain_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.starts_with('.')
        && !s.contains(['/', '\\', ':'])
        && !s.chars().any(char::is_control)
}

fn read_to_string(path: &Path) -> Result<String, OracleError> {
    std::fs::read_to_string(path).map_err(|source| OracleError::Io {
        path: path.to_owned(),
        source,
    })
}

fn invalid(msg: String) -> OracleError {
    OracleError::Invalid(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names() {
        assert!(is_plain_name("roads.shp"));
        assert!(is_plain_name("ortho.tif.aux.xml"));
        for bad in [
            "", ".", "..", "../x", "a/b", "a\\b", "C:x", ".hidden", "a\nb",
        ] {
            assert!(!is_plain_name(bad), "{bad:?} accepted");
        }
    }

    #[test]
    fn manifest_rejects_unknown_keys_and_bad_cases() {
        let ok = r#"
            schema = 1
            [[case]]
            id = "a"
            format = "shapefile"
            origin = "synthetic"
            anchor = "a.shp"
            files = ["a.shp"]
            description = "d"
            provenance = "p"
            licence = "MIT"
        "#;
        let m: Manifest = toml::from_str(ok).expect("valid manifest");
        assert_eq!(m.cases[0].location, Location::Committed);

        let unknown = ok.replace("licence = \"MIT\"", "licence = \"MIT\"\nlicense = \"x\"");
        assert!(toml::from_str::<Manifest>(&unknown).is_err());

        let dir = tempfile::tempdir().expect("temp dir");
        let traversal = ok.replace("files = [\"a.shp\"]", "files = [\"a.shp\", \"../x\"]");
        std::fs::write(dir.path().join("manifest.toml"), traversal).expect("write manifest");
        let err = Corpus::load(dir.path()).expect_err("path traversal must be rejected");
        assert!(err.to_string().contains("plain file name"), "{err}");

        let dup = format!("{ok}\n{}", &ok[ok.find("[[case]]").expect("case table")..]);
        std::fs::write(dir.path().join("manifest.toml"), dup).expect("write manifest");
        let err = Corpus::load(dir.path()).expect_err("duplicate ids must be rejected");
        assert!(err.to_string().contains("duplicate"), "{err}");
    }
}
