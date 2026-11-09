//! The stage01 gate cannot pass by declining data or silently removing CRS checks.
use whereabouts_oracle::{Check, Corpus, adapters, corpus_dir, evaluate};

#[test]
fn declared_crs_checks_cannot_be_removed_to_bypass_the_baseline() {
    let corpus = Corpus::load(corpus_dir()).expect("corpus");
    let mut registry = adapters::registry().to_vec();
    registry[0].checks = &[Check::Status, Check::Layers, Check::NativeExtent];
    let report = evaluate(&corpus, &registry, &Default::default()).expect("report");
    let failures = report.coverage_failures(&corpus, &registry);
    assert!(failures.iter().any(|s| s.contains("epsg")), "{failures:?}");
    assert!(
        failures.iter().any(|s| s.contains("wgs84_extent")),
        "{failures:?}"
    );
}

#[test]
fn extraction_preserves_transient_and_malformed_diagnostics() {
    let corpus = Corpus::load(corpus_dir()).expect("corpus");
    for adapter in adapters::registry() {
        let case = corpus
            .cases()
            .iter()
            .find(|c| c.format == adapter.format && c.is_corrupt())
            .expect("corrupt case");
        let missing = tempfile::tempdir().expect("empty directory");
        let transient = (adapter.read)(&adapters::ResolvedCase::new(case, missing.path().into()))
            .expect("record");
        assert!(transient.retry, "{}: {transient:?}", case.id);
        assert!(transient.reason.is_some(), "{}", case.id);
        let malformed = (adapter.read)(&adapters::ResolvedCase::new(case, corpus.case_dir(case)))
            .expect("record");
        assert!(!malformed.retry, "{}: {malformed:?}", case.id);
        assert!(malformed.reason.is_some(), "{}", case.id);
    }
}
