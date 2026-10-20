//! Per-format agreement report.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use crate::compare::{Check, CheckResult, Verdict};

/// Why a case was not compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    /// No adapter is registered for the case's format.
    NoAdapter,
    /// The adapter returned `None` for this case.
    AdapterDeclined,
    /// The case's files are absent (external sample not fetched).
    DataMissing,
}

impl SkipReason {
    fn label(self) -> &'static str {
        match self {
            SkipReason::NoAdapter => "no adapter",
            SkipReason::AdapterDeclined => "declined",
            SkipReason::DataMissing => "data missing",
        }
    }
}

/// Result of running one case.
#[derive(Debug, Clone, PartialEq)]
pub enum CaseOutcome {
    /// Compared; one result per applicable declared check.
    Compared(Vec<CheckResult>),
    /// Not compared.
    Skipped(SkipReason),
    /// The adapter panicked (message). Always a failure.
    Panicked(String),
}

/// One case's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseReport {
    /// Case id.
    pub case: String,
    /// Format directory name.
    pub format: String,
    /// What happened.
    pub outcome: CaseOutcome,
}

/// Agreement count for one check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Results that agreed.
    pub agreed: u32,
    /// Results compared.
    pub compared: u32,
}

impl Tally {
    /// `agreed / compared`, or `None` when nothing was compared.
    pub fn rate(self) -> Option<f64> {
        (self.compared > 0).then(|| f64::from(self.agreed) / f64::from(self.compared))
    }
}

/// Summary for one format.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormatSummary {
    /// Cases in the corpus for this format.
    pub cases: u32,
    /// Cases compared.
    pub compared: u32,
    /// Cases skipped, by reason.
    pub skipped: BTreeMap<SkipReason, u32>,
    /// Cases whose adapter panicked.
    pub panicked: u32,
    /// Per-check tallies.
    pub checks: BTreeMap<Check, Tally>,
}

/// The whole run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Every case, in manifest order.
    pub cases: Vec<CaseReport>,
}

impl Report {
    /// Record one case's outcome.
    pub fn push(&mut self, case: String, format: String, outcome: CaseOutcome) {
        self.cases.push(CaseReport {
            case,
            format,
            outcome,
        });
    }

    /// Per-format summaries, keyed by format name.
    pub fn by_format(&self) -> BTreeMap<String, FormatSummary> {
        let mut out: BTreeMap<String, FormatSummary> = BTreeMap::new();
        for c in &self.cases {
            let s = out.entry(c.format.clone()).or_default();
            s.cases += 1;
            match &c.outcome {
                CaseOutcome::Skipped(r) => *s.skipped.entry(*r).or_default() += 1,
                CaseOutcome::Panicked(_) => s.panicked += 1,
                CaseOutcome::Compared(results) => {
                    s.compared += 1;
                    for r in results {
                        let t = s.checks.entry(r.check).or_default();
                        t.compared += 1;
                        t.agreed += u32::from(r.verdict.agrees());
                    }
                }
            }
        }
        out
    }

    /// Cases whose adapter panicked: `(case, message)`.
    pub fn panics(&self) -> Vec<(&str, &str)> {
        self.cases
            .iter()
            .filter_map(|c| match &c.outcome {
                CaseOutcome::Panicked(m) => Some((c.case.as_str(), m.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Every disagreement: `(case, result)`.
    pub fn disagreements(&self) -> Vec<(&str, &CheckResult)> {
        let mut out = Vec::new();
        for c in &self.cases {
            if let CaseOutcome::Compared(results) = &c.outcome {
                out.extend(
                    results
                        .iter()
                        .filter(|r| !r.verdict.agrees())
                        .map(|r| (c.case.as_str(), r)),
                );
            }
        }
        out
    }

    /// Markdown rendering (also used by `Display`).
    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "## Oracle agreement\n");
        let _ = write!(s, "| format | cases | compared | skipped |");
        for c in Check::ALL {
            let _ = write!(s, " {c} |");
        }
        let _ = write!(s, "\n|---|---:|---:|---|");
        for _ in Check::ALL {
            let _ = write!(s, "---:|");
        }
        s.push('\n');
        for (format, sum) in self.by_format() {
            let skipped = if sum.skipped.is_empty() {
                "0".to_owned()
            } else {
                sum.skipped
                    .iter()
                    .map(|(r, n)| format!("{n} {}", r.label()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = write!(
                s,
                "| {format} | {} | {} | {skipped} |",
                sum.cases, sum.compared
            );
            for c in Check::ALL {
                let cell = match sum
                    .checks
                    .get(&c)
                    .copied()
                    .and_then(|t| t.rate().map(|r| (t, r)))
                {
                    Some((t, r)) => format!("{}/{} ({:.1}%)", t.agreed, t.compared, r * 100.0),
                    None => "—".to_owned(),
                };
                let _ = write!(s, " {cell} |");
            }
            s.push('\n');
        }
        let panics = self.panics();
        if !panics.is_empty() {
            let _ = writeln!(s, "\n### Adapter panics\n");
            for (case, msg) in panics {
                let _ = writeln!(s, "- `{case}`: {msg}");
            }
        }
        let dis = self.disagreements();
        if !dis.is_empty() {
            let _ = writeln!(s, "\n### Disagreements\n");
            for (case, r) in dis {
                let layer = r
                    .layer
                    .as_deref()
                    .map(|l| format!(" `{l}`"))
                    .unwrap_or_default();
                if let Verdict::Disagree(why) = &r.verdict {
                    let _ = writeln!(s, "- `{case}`{layer} {}: {why}", r.check);
                }
            }
        }
        s
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_markdown())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tallies_and_markdown() {
        let mut r = Report::default();
        r.push(
            "a".into(),
            "shapefile".into(),
            CaseOutcome::Compared(vec![
                CheckResult {
                    check: Check::Status,
                    layer: None,
                    verdict: Verdict::Agree,
                },
                CheckResult {
                    check: Check::FeatureCount,
                    layer: Some("pts".into()),
                    verdict: Verdict::Disagree("expected Some(3), got Some(2)".into()),
                },
            ]),
        );
        r.push(
            "b".into(),
            "shapefile".into(),
            CaseOutcome::Skipped(SkipReason::AdapterDeclined),
        );
        r.push(
            "c".into(),
            "geotiff".into(),
            CaseOutcome::Skipped(SkipReason::NoAdapter),
        );
        let by = r.by_format();
        let shp = &by["shapefile"];
        assert_eq!((shp.cases, shp.compared), (2, 1));
        assert_eq!(shp.checks[&Check::Status].rate(), Some(1.0));
        assert_eq!(shp.checks[&Check::FeatureCount].rate(), Some(0.0));
        assert_eq!(by["geotiff"].skipped[&SkipReason::NoAdapter], 1);
        let md = r.to_markdown();
        assert!(md.contains("| shapefile | 2 | 1 | 1 declined |"), "{md}");
        assert!(md.contains("1/1 (100.0%)"), "{md}");
        assert!(
            md.contains("- `a` `pts` feature_count: expected Some(3)"),
            "{md}"
        );
        assert_eq!(r.disagreements().len(), 1);
    }
}
