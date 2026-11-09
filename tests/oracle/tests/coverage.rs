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

#[test]
fn cp437_field_name_disagreement_is_explicit_partial_per_d42() {
    use whereabouts_core::{Format, MemberRole, ReadLimits, ResolvedDataset, ResolvedMember};
    let corpus = Corpus::load(corpus_dir()).expect("corpus");
    let case = corpus.case("shp_cp437_names").expect("case");
    let dir = corpus.case_dir(case);
    let dataset = ResolvedDataset {
        format: Format::Shapefile,
        anchor: dir.join(&case.anchor),
        sidecars: [
            ("places.dbf", MemberRole::Dbf),
            ("places.shx", MemberRole::Shx),
            ("places.prj", MemberRole::Prj),
            ("places.cpg", MemberRole::Cpg),
        ]
        .into_iter()
        .map(|(name, role)| ResolvedMember {
            role,
            path: dir.join(name),
        })
        .collect(),
    };
    let extracted = whereabouts_indexer::extract(&dataset, &ReadLimits::default());
    let golden = corpus.load_expected(case).expect("golden");
    let actual: Vec<_> = extracted.layers[0]
        .metadata
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let expected: Vec<_> = golden.layers[0]
        .fields
        .as_ref()
        .expect("fields")
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    // D-42/#42 explicitly retain this unsupported DOS-code-page fallback. Do not
    // count the discrepancy as field-name agreement with GDAL.
    assert_eq!(expected, ["id", "café"]);
    assert_eq!(actual, ["id", "caf‚"]);
    let whereabouts_core::ExtractionStatus::Partial(reason) = extracted.status else {
        panic!("unsupported non-ASCII encoding must be partial");
    };
    assert!(
        reason.contains("CP437") && reason.contains("Windows-1252"),
        "{reason}"
    );
}
