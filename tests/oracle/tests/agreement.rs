//! Oracle agreement gate (no libtest harness, so the report always prints).
//!
//! Runs every corpus case through its registered adapter, prints the per-format agreement report
//! (also appended to `$GITHUB_STEP_SUMMARY` in CI) and fails if any rate is below
//! `tests/oracle/baseline.toml` or any adapter panicked.

use std::io::Write as _;
use std::process::ExitCode;

use whereabouts_oracle::{Baseline, Corpus, adapters, baseline_path, corpus_dir, evaluate};

fn main() -> ExitCode {
    // libtest-style invocations (`cargo test -- --list`, filters) are irrelevant here: this
    // binary has exactly one test, which always runs.
    if std::env::args().any(|a| a == "--list") {
        println!("agreement: test");
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("oracle agreement: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let corpus = Corpus::load(corpus_dir())?;
    let baseline = Baseline::load(&baseline_path())?;
    let report = evaluate(&corpus, adapters::registry(), &baseline.tolerance)?;

    let mut text = report.to_markdown();
    let violations = baseline.violations(&report);
    let panics = report.panics();
    let passed = violations.is_empty() && panics.is_empty();
    if !violations.is_empty() {
        text.push_str("\n### Below baseline\n\n");
        for v in &violations {
            text.push_str(&format!("- {v}\n"));
        }
    }
    text.push_str(&format!(
        "\nresult: {} ({} cases, {} adapters registered)\n",
        if passed { "PASS" } else { "FAIL" },
        corpus.cases().len(),
        adapters::registry().len()
    ));
    println!("{text}");

    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY")
        && let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(summary)
    {
        let _ = writeln!(f, "{text}");
    }
    Ok(passed)
}
