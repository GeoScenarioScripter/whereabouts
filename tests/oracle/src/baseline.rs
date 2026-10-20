//! `tests/oracle/baseline.toml`: comparison tolerances and the minimum agreement rates CI enforces.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::OracleError;
use crate::compare::{Check, Tolerances};
use crate::manifest::KNOWN_FORMATS;
use crate::report::Report;

/// Parsed baseline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    /// Comparison tolerances.
    pub tolerance: Tolerances,
    /// Default minimum agreement rate per check (0.0–1.0). Missing checks default to 0.
    #[serde(default)]
    pub min_rate: BTreeMap<Check, f64>,
    /// Per-format overrides.
    #[serde(default)]
    pub format: BTreeMap<String, FormatBaseline>,
}

/// Per-format overrides.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatBaseline {
    /// Minimum agreement rate per check for this format, overriding the default.
    #[serde(default)]
    pub min_rate: BTreeMap<Check, f64>,
}

/// A per-format, per-check rate below its baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    /// Format.
    pub format: String,
    /// Check.
    pub check: Check,
    /// Results that agreed.
    pub agreed: u32,
    /// Results compared.
    pub compared: u32,
    /// The minimum it had to meet.
    pub min_rate: f64,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} / {}: {}/{} agree ({:.1}%), baseline requires {:.1}%",
            self.format,
            self.check,
            self.agreed,
            self.compared,
            100.0 * f64::from(self.agreed) / f64::from(self.compared.max(1)),
            100.0 * self.min_rate
        )
    }
}

impl Baseline {
    /// Load and validate a baseline file.
    pub fn load(path: &Path) -> Result<Self, OracleError> {
        let text = std::fs::read_to_string(path).map_err(|source| OracleError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text).map_err(|e| match e {
            OracleError::Invalid(m) => OracleError::Invalid(format!("{}: {m}", path.display())),
            other => other,
        })
    }

    /// Parse and validate baseline TOML.
    pub fn parse(text: &str) -> Result<Self, OracleError> {
        let b: Baseline = toml::from_str(text).map_err(|source| OracleError::Toml {
            path: "baseline.toml".into(),
            source,
        })?;
        // A typo'd format section would silently enforce nothing; unknown check names are
        // already rejected by `Check`'s Deserialize.
        if let Some(unknown) = b
            .format
            .keys()
            .find(|f| !KNOWN_FORMATS.contains(&f.as_str()))
        {
            return Err(OracleError::Invalid(format!(
                "[format.{unknown}] is not a known format (known: {})",
                KNOWN_FORMATS.join(", ")
            )));
        }
        let rates = b
            .min_rate
            .iter()
            .chain(b.format.values().flat_map(|f| f.min_rate.iter()));
        for (check, rate) in rates {
            if !(0.0..=1.0).contains(rate) {
                return Err(OracleError::Invalid(format!(
                    "min_rate.{check} = {rate} is outside 0.0–1.0"
                )));
            }
        }
        let t = &b.tolerance;
        for (name, v) in [
            ("native_abs", t.native_abs),
            ("native_rel", t.native_rel),
            ("wgs84_deg", t.wgs84_deg),
            ("resolution_rel", t.resolution_rel),
        ] {
            if !(v.is_finite() && v >= 0.0) {
                return Err(OracleError::Invalid(format!(
                    "tolerance.{name} = {v} must be finite and >= 0"
                )));
            }
        }
        Ok(b)
    }

    /// Minimum rate for `check` on `format`.
    pub fn min_rate(&self, format: &str, check: Check) -> f64 {
        self.format
            .get(format)
            .and_then(|f| f.min_rate.get(&check))
            .or_else(|| self.min_rate.get(&check))
            .copied()
            .unwrap_or(0.0)
    }

    /// Every per-format, per-check rate below its minimum. Checks with nothing compared are not
    /// enforced (an empty adapter registry passes).
    pub fn violations(&self, report: &Report) -> Vec<Violation> {
        let mut out = Vec::new();
        for (format, summary) in report.by_format() {
            for (check, tally) in &summary.checks {
                let Some(rate) = tally.rate() else { continue };
                let min = self.min_rate(&format, *check);
                if rate < min {
                    out.push(Violation {
                        format: format.clone(),
                        check: *check,
                        agreed: tally.agreed,
                        compared: tally.compared,
                        min_rate: min,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{CheckResult, Verdict};
    use crate::report::CaseOutcome;

    const TOML: &str = r#"
        [tolerance]
        native_abs = 1e-6
        native_rel = 1e-9
        wgs84_deg = 0.01
        resolution_rel = 1e-9

        [min_rate]
        feature_count = 0.98
        epsg = 0.0

        [format.geotiff.min_rate]
        feature_count = 0.5
    "#;

    #[test]
    fn overrides_and_violations() {
        let b = Baseline::parse(TOML).expect("valid baseline");
        assert_eq!(b.min_rate("shapefile", Check::FeatureCount), 0.98);
        assert_eq!(b.min_rate("geotiff", Check::FeatureCount), 0.5);
        assert_eq!(b.min_rate("shapefile", Check::Raster), 0.0);

        let mut r = Report::default();
        let result = |agree| CheckResult {
            check: Check::FeatureCount,
            layer: None,
            verdict: if agree {
                Verdict::Agree
            } else {
                Verdict::Disagree("x".into())
            },
        };
        r.push(
            "a".into(),
            "shapefile".into(),
            CaseOutcome::Compared(vec![result(true), result(false)]),
        );
        r.push(
            "b".into(),
            "geotiff".into(),
            CaseOutcome::Compared(vec![result(true), result(false)]),
        );
        let v = b.violations(&r);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].format, "shapefile");
        assert!(v[0].to_string().contains("1/2 agree (50.0%)"));
        assert!(b.violations(&Report::default()).is_empty());
    }

    #[test]
    fn rejects_bad_values_and_unknown_checks() {
        assert!(Baseline::parse(&TOML.replace("epsg = 0.0", "epsg = 1.5")).is_err());
        assert!(Baseline::parse(&TOML.replace("epsg = 0.0", "epgs = 0.5")).is_err());
        assert!(Baseline::parse(&TOML.replace("wgs84_deg = 0.01", "wgs84_deg = -1.0")).is_err());
        // Unknown check name inside a format section, and an unknown format section.
        assert!(
            Baseline::parse(&TOML.replace("feature_count = 0.5", "feature_cnt = 0.5")).is_err()
        );
        let err = Baseline::parse(&TOML.replace("[format.geotiff.", "[format.geotif."))
            .expect_err("unknown format must be rejected");
        assert!(err.to_string().contains("geotif"), "{err}");
    }
}
