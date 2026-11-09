//! Raster facts beyond the generic oracle record, with a counting-I/O assertion.

use whereabouts_core::{Format, MemberRole, ReadLimits, ResolvedDataset, ResolvedMember};
use whereabouts_oracle::{Corpus, corpus_dir};

#[test]
fn grouper_supplies_every_corpus_tiff_member_with_its_reader_role() {
    use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn, VolumeId};
    use whereabouts_datasets::{Grouper, MemoryTree};
    let corpus = Corpus::load(corpus_dir()).expect("corpus");
    for case in corpus.cases().iter().filter(|c| c.format == "geotiff") {
        let mut tree = MemoryTree::new();
        let root = Frn(5);
        tree.upsert(&FileRecord {
            frn: root,
            parent: root,
            name: FileName::from(""),
            attrs: FileAttributes(FileAttributes::DIRECTORY),
        });
        for (i, name) in case.files.iter().enumerate() {
            tree.upsert(&FileRecord {
                frn: Frn(i as u64 + 100),
                parent: root,
                name: FileName::from(name.as_str()),
                attrs: FileAttributes::default(),
            });
        }
        let mut grouper = Grouper::new(VolumeId(1));
        grouper.full_scan(&tree, root);
        let datasets: Vec<_> = grouper.datasets().collect();
        assert_eq!(datasets.len(), 1, "{}", case.id);
        assert_eq!(datasets[0].format(), Format::GeoTiff);
        assert_eq!(datasets[0].members().len(), case.files.len(), "{}", case.id);
        for member in datasets[0].members() {
            let name = &case.files[(member.frn.0 - 100) as usize];
            let expected = if *name == case.anchor {
                MemberRole::Main
            } else if name.ends_with(".aux.xml") {
                MemberRole::AuxXml
            } else if name.ends_with(".prj") {
                MemberRole::Prj
            } else if name.ends_with(".tfw") {
                MemberRole::WorldFile
            } else if name.ends_with(".ovr") {
                MemberRole::Ovr
            } else if name.ends_with(".msk") {
                MemberRole::Msk
            } else {
                MemberRole::Other
            };
            assert_eq!(member.role, expected, "{}: {name}", case.id);
        }
    }
}

#[test]
fn all_tiff_raster_facts_match_gdal_with_bounded_io() {
    let corpus = Corpus::load(corpus_dir()).expect("corpus");
    let mut count = 0;
    for case in corpus.cases().iter().filter(|c| c.format == "geotiff") {
        let dir = corpus.case_dir(case);
        let ds = ResolvedDataset {
            format: Format::GeoTiff,
            anchor: dir.join(&case.anchor),
            sidecars: case
                .files
                .iter()
                .filter(|p| **p != case.anchor)
                .map(|p| ResolvedMember {
                    path: dir.join(p),
                    role: if p.ends_with(".aux.xml") {
                        MemberRole::AuxXml
                    } else if p.ends_with(".tfw") {
                        MemberRole::WorldFile
                    } else if p.ends_with(".prj") {
                        MemberRole::Prj
                    } else if p.ends_with(".ovr") {
                        MemberRole::Ovr
                    } else if p.ends_with(".msk") {
                        MemberRole::Msk
                    } else {
                        MemberRole::Other
                    },
                })
                .collect(),
        };
        let (result, stats) =
            whereabouts_readers::measure(|| whereabouts_readers::read(&ds, &ReadLimits::default()));
        println!(
            "{}: {} bytes, {} reads",
            case.id, stats.bytes_read, stats.reads
        );
        assert!(stats.bytes_read < 64 * 1024, "{}: {stats:?}", case.id);
        if case.is_corrupt() {
            assert!(result.is_err(), "{}", case.id);
            continue;
        }
        let md = result.unwrap_or_else(|e| panic!("{}: {e}", case.id));
        assert!(md.partial.is_empty(), "{}: {:?}", case.id, md.partial);
        let golden = corpus.load_expected(case).expect("golden");
        let actual = md.layers[0].raster.as_ref().expect("raster");
        let expected = golden.layers[0].raster.as_ref().expect("oracle raster");
        assert_eq!(
            actual.overview_count, expected.overview_count,
            "{}",
            case.id
        );
        match (actual.geotransform, expected.geotransform) {
            (Some(a), Some(b)) => {
                for (a, b) in a.into_iter().zip(b) {
                    assert!((a - b).abs() <= 1e-6, "{}: {a} != {b}", case.id);
                }
            }
            (a, b) => assert_eq!(a, b, "{}", case.id),
        }
        count += 1;
    }
    assert_eq!(
        count,
        corpus
            .cases()
            .iter()
            .filter(|c| c.format == "geotiff" && !c.is_corrupt())
            .count()
    );
}
