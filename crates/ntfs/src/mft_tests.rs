//! Tests for the raw `$MFT` reader: hand-built fixtures, hostile inputs, mutation fuzzing.
//! Everything here is pure and runs on every platform.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ops::ControlFlow;

use crate::mft::{
    ATTR_ATTRIBUTE_LIST, ATTR_DATA, ATTR_FILE_NAME, ATTR_STANDARD_INFORMATION, AttrBody,
    AttrListEntries, Attrs, MAX_RUNS, MftParseError, NAMESPACE_DOS, NAMESPACE_POSIX,
    NAMESPACE_WIN32, NAMESPACE_WIN32_AND_DOS, RECORD_IN_USE, Run, apply_fixups, decode_runlist,
    parse_file_name, parse_record_header, parse_standard_info, parse_volume_data,
};
use crate::mft_build::{
    ImageBuilder, attr_list_entry, encode_runs, file_name_value, nonresident_attr, record,
    reference, resident_attr, seal, simple_file, standard_info, volume_data,
};
use crate::mft_scan::{MftError, MftScanOptions, MftScanStats, MftScanner};
use crate::record::{FILE_ATTRIBUTE_DIRECTORY, RawRecord};

const ROOT: u64 = reference(5, 5);

/// Deterministic xorshift so the fuzz-style tests need no extra dependency.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn scan_image(img: &[u8], b: &ImageBuilder) -> Result<(Vec<RawRecord>, MftScanStats), MftError> {
    scan_with(img, b, &MftScanOptions::default())
}

fn scan_with(
    img: &[u8],
    b: &ImageBuilder,
    options: &MftScanOptions,
) -> Result<(Vec<RawRecord>, MftScanStats), MftError> {
    let scanner = MftScanner::open(img, b.geometry())?;
    let mut out = Vec::new();
    let stats = scanner.scan(options, &mut |r| {
        out.push(r);
        ControlFlow::Continue(())
    })?;
    Ok((out, stats))
}

/// Root, `gis` directory, `a.shp` and `b.dbf` inside it.
fn tree() -> ImageBuilder {
    let mut b = ImageBuilder::new(4096, 1024, 64, 4);
    b.put(5, simple_file(1024, 5, true, ROOT, ".", 0x10));
    b.put(6, simple_file(1024, 2, true, ROOT, "gis", 0x10));
    b.put(
        7,
        simple_file(1024, 3, false, reference(6, 2), "a.shp", 0x20),
    );
    b.put(
        8,
        simple_file(1024, 1, false, reference(6, 2), "東京.dbf", 0x20),
    );
    b
}

fn by_name<'a>(records: &'a [RawRecord], name: &str) -> &'a RawRecord {
    records
        .iter()
        .find(|r| r.name.to_string_lossy() == name)
        .unwrap_or_else(|| panic!("no record named {name}: {records:?}"))
}

// ---- volume data -----------------------------------------------------------------------------

fn geometry() -> crate::mft::VolumeGeometry {
    ImageBuilder::new(4096, 1024, 64, 4).geometry()
}

#[test]
fn volume_data_round_trips() {
    let g = geometry();
    assert_eq!(parse_volume_data(&volume_data(&g)).unwrap(), g);
}

#[test]
fn volume_data_rejects_impossible_geometry() {
    let good = volume_data(&geometry());
    assert!(parse_volume_data(&good[..95]).is_err());
    for (offset, value) in [
        (40_usize, 0_u32),
        (40, 3),
        (40, 8192),
        (44, 100),
        (44, 256),
        (48, 0),
        (48, 768),
        (48, 1 << 20),
    ] {
        let mut bad = good.clone();
        bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(parse_volume_data(&bad).is_err(), "{offset} = {value}");
    }
    let mut bad = good.clone();
    bad[16..24].copy_from_slice(&0_u64.to_le_bytes());
    assert!(parse_volume_data(&bad).is_err());
    let mut bad = good.clone();
    bad[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(parse_volume_data(&bad).is_err());
    let mut bad = good;
    bad[64..72].copy_from_slice(&64_u64.to_le_bytes()); // MFT at the end of the volume
    assert!(parse_volume_data(&bad).is_err());
}

// ---- fixups ----------------------------------------------------------------------------------

#[test]
fn fixups_restore_the_sector_tails() {
    // The attribute's value covers the first sector's last two bytes (510..512).
    let payload = resident_attr(0x40, "", &[0xAA; 600]);
    let mut r = record(1024, 1, RECORD_IN_USE, 0, &[payload]);
    // The sector tails now hold the update sequence number.
    assert_eq!(&r[510..512], &crate::mft_build::USN.to_le_bytes());
    assert_eq!(&r[1022..1024], &crate::mft_build::USN.to_le_bytes());
    apply_fixups(&mut r).unwrap();
    assert_eq!(&r[510..512], &[0xAA, 0xAA]);
    assert_eq!(&r[1022..1024], &[0, 0]);
}

#[test]
fn fixups_reject_torn_and_malformed_records() {
    let good = record(1024, 1, RECORD_IN_USE, 0, &[]);
    let mut torn = good.clone();
    torn[1022] ^= 0xFF;
    assert_eq!(
        apply_fixups(&mut torn),
        Err(MftParseError::TornWrite { sector: 1 })
    );
    let mut torn = good.clone();
    torn[510] ^= 1;
    assert_eq!(
        apply_fixups(&mut torn),
        Err(MftParseError::TornWrite { sector: 0 })
    );
    let mut zeroed = vec![0_u8; 1024];
    assert_eq!(apply_fixups(&mut zeroed), Err(MftParseError::NotFileRecord));
    let mut baad = good.clone();
    baad[..4].copy_from_slice(b"BAAD");
    assert_eq!(apply_fixups(&mut baad), Err(MftParseError::NotFileRecord));
    for (at, value) in [
        (4_usize, 0_u16),
        (4, 0x1FE),
        (4, 0xFFFF),
        (6, 1),
        (6, 2),
        (6, 4),
        (6, 0xFFFF),
    ] {
        let mut bad = good.clone();
        bad[at..at + 2].copy_from_slice(&value.to_le_bytes());
        assert!(
            matches!(apply_fixups(&mut bad), Err(MftParseError::Malformed { .. })),
            "{at} = {value:#x}"
        );
    }
    assert!(apply_fixups(&mut good.clone()[..1000]).is_err());
    assert!(apply_fixups(&mut []).is_err());
}

#[test]
fn record_header_is_validated() {
    let mut r = record(1024, 7, RECORD_IN_USE, 0, &[]);
    apply_fixups(&mut r).unwrap();
    let h = parse_record_header(&r).unwrap();
    assert_eq!(h.sequence, 7);
    assert!(h.in_use() && !h.is_directory() && !h.is_extension());
    for (at, value) in [
        (0x14_usize, 0_u32),
        (0x14, 0x2F),
        (0x14, 4000),
        (0x14, 0x3C),
        (0x18, 5000),
        (0x18, 0x20),
    ] {
        let mut bad = r.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(parse_record_header(&bad).is_err(), "{at:#x} = {value:#x}");
    }
    assert!(parse_record_header(&r[..0x20]).is_err());
}

// ---- attributes ------------------------------------------------------------------------------

fn fixed_up(attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut r = record(1024, 1, RECORD_IN_USE, 0, attrs);
    apply_fixups(&mut r).unwrap();
    r
}

#[test]
fn attributes_parse_resident_and_non_resident() {
    let r = fixed_up(&[
        resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(5, 0x20, 77)),
        resident_attr(
            ATTR_FILE_NAME,
            "",
            &file_name_value(ROOT, NAMESPACE_WIN32, "x.tif"),
        ),
        nonresident_attr(
            ATTR_DATA,
            "stream",
            0,
            10_000,
            10_000,
            &encode_runs(&[(3, Some(9))]),
        ),
    ]);
    let h = parse_record_header(&r).unwrap();
    let attrs: Vec<_> = Attrs::new(&r, &h).map(Result::unwrap).collect();
    assert_eq!(attrs.len(), 3);
    let AttrBody::Resident { value } = attrs[0].body else {
        panic!()
    };
    let si = parse_standard_info(value).unwrap();
    assert_eq!((si.attributes, si.usn, si.modified), (0x20, 77, 5));
    let AttrBody::Resident { value } = attrs[1].body else {
        panic!()
    };
    let name = parse_file_name(value).unwrap();
    assert_eq!(name.parent, ROOT);
    assert_eq!(name.namespace, NAMESPACE_WIN32);
    assert_eq!(name.name.len(), 10);
    assert!(!attrs[2].is_unnamed());
    let AttrBody::NonResident {
        real_size, runlist, ..
    } = attrs[2].body
    else {
        panic!()
    };
    assert_eq!(real_size, 10_000);
    let mut runs = Vec::new();
    assert_eq!(decode_runlist(runlist, 0, 100, &mut runs).unwrap(), 3);
    assert_eq!(
        runs,
        [Run {
            vcn: 0,
            len: 3,
            lcn: Some(9)
        }]
    );
}

#[test]
fn attribute_length_lies_end_the_chain_with_one_error() {
    let good = resident_attr(ATTR_FILE_NAME, "", &file_name_value(ROOT, 1, "a"));
    for lie in [0_u32, 8, 0x17, 0x19, 0x1_0000, u32::MAX, 0x7FFF_FFF8] {
        let mut bad = good.clone();
        bad[4..8].copy_from_slice(&lie.to_le_bytes());
        let mut r = record(1024, 1, RECORD_IN_USE, 0, std::slice::from_ref(&good));
        let first = usize::from(u16::from_le_bytes([r[0x14], r[0x15]]));
        r[first..first + bad.len()].copy_from_slice(&bad);
        // Re-seal: the first sector tail is untouched, so just parse without fixups.
        let h = parse_record_header(&r).unwrap();
        let items: Vec<_> = Attrs::new(&r, &h).collect();
        assert!(items.last().is_some_and(Result::is_err), "lie {lie:#x}");
        assert_eq!(items.len(), 1);
    }
    // Value and name offsets pointing outside the attribute.
    for (at, value) in [
        (0x14_usize, 0xFFF0_u16),
        (0x14, 0x10),
        (10, 0xFFF0),
        (0x10, 0xFFFF),
    ] {
        let mut bad = good.clone();
        if at == 0x10 {
            bad[at..at + 4].copy_from_slice(&u32::from(value).to_le_bytes());
        } else {
            bad[at..at + 2].copy_from_slice(&value.to_le_bytes());
            bad[9] = 4; // a name, so name_off matters
        }
        let mut r = record(1024, 1, RECORD_IN_USE, 0, std::slice::from_ref(&good));
        let first = usize::from(u16::from_le_bytes([r[0x14], r[0x15]]));
        r[first..first + bad.len()].copy_from_slice(&bad);
        let h = parse_record_header(&r).unwrap();
        assert!(
            Attrs::new(&r, &h).any(|a| a.is_err()),
            "{at:#x} = {value:#x}"
        );
    }
}

// ---- runlists --------------------------------------------------------------------------------

#[test]
fn runlists_decode_sparse_negative_and_multi_byte_fields() {
    let bytes = encode_runs(&[
        (5, Some(1000)),
        (3, None),
        (300, Some(70)),
        (1, Some(100_000)),
    ]);
    let mut runs = Vec::new();
    let clusters = decode_runlist(&bytes, 10, 1 << 20, &mut runs).unwrap();
    assert_eq!(clusters, 309);
    assert_eq!(
        runs,
        [
            Run {
                vcn: 10,
                len: 5,
                lcn: Some(1000)
            },
            Run {
                vcn: 15,
                len: 3,
                lcn: None
            },
            Run {
                vcn: 18,
                len: 300,
                lcn: Some(70)
            },
            Run {
                vcn: 318,
                len: 1,
                lcn: Some(100_000)
            },
        ]
    );
}

#[test]
fn runlists_reject_hostile_encodings() {
    let mut runs = Vec::new();
    let cases: &[(&str, &[u8])] = &[
        ("zero length", &[0x11, 0x00, 0x05, 0x00]),
        ("length size 0", &[0x10, 0x05, 0x00]),
        (
            "length size 9",
            &[0x19, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0x05, 0x00],
        ),
        (
            "offset size 9",
            &[0x91, 0x01, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0x00],
        ),
        ("truncated fields", &[0x42, 0x05]),
        ("negative lcn", &[0x11, 0x05, 0xF0, 0x00]),
        ("beyond the volume", &[0x11, 0x05, 0x70, 0x00]),
        (
            "huge length",
            &[
                0x18, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x01, 0x00,
            ],
        ),
    ];
    for (what, bytes) in cases {
        runs.clear();
        assert!(
            decode_runlist(bytes, 0, 100, &mut runs).is_err(),
            "{what} was accepted: {runs:?}"
        );
    }
    // A VCN overflow.
    runs.clear();
    assert!(decode_runlist(&[0x11, 0x05, 0x01, 0x00], u64::MAX - 2, 100, &mut runs).is_err());
}

#[test]
fn runlist_bomb_is_capped() {
    // Two bytes per run, all sparse: a 2 MiB attribute decodes to 1M runs -> the cap.
    let bytes: Vec<u8> = std::iter::repeat_n([0x01_u8, 0x01], MAX_RUNS + 10)
        .flatten()
        .collect();
    let mut runs = Vec::new();
    let err = decode_runlist(&bytes, 0, 100, &mut runs).unwrap_err();
    assert!(matches!(err, MftParseError::Limit { .. }), "{err:?}");
    assert_eq!(runs.len(), MAX_RUNS);
}

#[test]
fn attribute_list_entries_validate_lengths() {
    let mut list = attr_list_entry(ATTR_FILE_NAME, 0, reference(9, 1));
    list.extend(attr_list_entry(ATTR_DATA, 0, reference(9, 1)));
    list.extend([0_u8; 3]); // padding
    let entries: Vec<_> = AttrListEntries::new(&list).map(Result::unwrap).collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].type_code, ATTR_FILE_NAME);
    for rec_len in [0_u16, 1, 0x19, 0x21, 0xFFFF] {
        let mut bad = list.clone();
        bad[4..6].copy_from_slice(&rec_len.to_le_bytes());
        let items: Vec<_> = AttrListEntries::new(&bad).collect();
        assert!(items.len() <= 2, "no progress guarantee for {rec_len}");
        if rec_len < 0x1A || usize::from(rec_len) > bad.len() {
            assert!(items[0].is_err(), "{rec_len:#x}");
        }
    }
}

// ---- whole-volume scans ----------------------------------------------------------------------

#[test]
fn scans_a_small_tree() {
    let b = tree();
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    // $MFT itself (record 0) plus the four records.
    assert_eq!(records.len(), 5, "{records:?}");
    let mft = by_name(&records, "$MFT");
    assert_eq!(mft.frn, reference(0, 1));
    assert_eq!(mft.parent_frn, ROOT);

    let root = by_name(&records, ".");
    assert_eq!(root.frn, ROOT);
    assert!(root.is_directory());

    let gis = by_name(&records, "gis");
    assert_eq!(gis.frn, reference(6, 2));
    assert_eq!(gis.parent_frn, ROOT);
    assert_ne!(gis.file_attributes & FILE_ATTRIBUTE_DIRECTORY, 0);

    let shp = by_name(&records, "a.shp");
    assert_eq!(shp.frn, reference(7, 3));
    assert_eq!(shp.parent_frn, reference(6, 2));
    assert_eq!(shp.file_attributes, 0x20);
    assert_eq!(shp.timestamp, 132_000_000_000_000_000);

    let tokyo = by_name(&records, "東京.dbf");
    assert_eq!(tokyo.frn, reference(8, 1));

    assert_eq!(stats.yielded, 5);
    assert_eq!(stats.lost(), 0);
    // 16 record slots: 5 used; the bitmap-clear slots are counted as unused, none examined.
    assert_eq!(stats.unused, 11);
    assert_eq!(stats.examined, 5);
    assert_eq!(stats.reads, 1);
}

#[test]
fn skip_reserved_hides_system_records_but_not_the_root() {
    let mut b = ImageBuilder::new(4096, 1024, 64, 8);
    b.put(5, simple_file(1024, 5, true, ROOT, ".", 0x10));
    b.put(11, simple_file(1024, 1, true, ROOT, "$Extend", 0x10));
    b.put(20, simple_file(1024, 3, false, ROOT, "a.shp", 0x20));
    let img = b.build();
    let opts = MftScanOptions {
        skip_reserved: true,
        ..MftScanOptions::default()
    };
    let (records, _) = scan_with(&img, &b, &opts).unwrap();
    let names: Vec<_> = records
        .iter()
        .map(|r| r.name.to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [".", "a.shp"]);
}

#[test]
fn a_directory_flag_in_the_header_wins_over_stale_attributes() {
    let mut b = tree();
    // Directory flag set in the header, attributes say "archive" only.
    b.put(9, simple_file(1024, 1, true, ROOT, "dir", 0x20));
    let img = b.build();
    let (records, _) = scan_image(&img, &b).unwrap();
    let dir = by_name(&records, "dir");
    assert_eq!(dir.file_attributes, 0x20 | FILE_ATTRIBUTE_DIRECTORY);
}

fn multi_name(names: &[(u64, u8, &str)]) -> Vec<u8> {
    let mut attrs = vec![resident_attr(
        ATTR_STANDARD_INFORMATION,
        "",
        &standard_info(1, 0x20, 0),
    )];
    for &(parent, ns, name) in names {
        attrs.push(resident_attr(
            ATTR_FILE_NAME,
            "",
            &file_name_value(parent, ns, name),
        ));
    }
    record(1024, 4, RECORD_IN_USE, 0, &attrs)
}

#[test]
fn name_selection_prefers_win32_and_never_reports_dos_names() {
    let mut b = tree();
    // Long Win32 name + its DOS alias (DOS first, as written by some drivers).
    b.put(
        9,
        multi_name(&[
            (reference(6, 2), NAMESPACE_DOS, "LONGFI~1.TIF"),
            (reference(6, 2), NAMESPACE_WIN32, "long file name.tif"),
        ]),
    );
    // A name valid in both namespaces.
    b.put(
        10,
        multi_name(&[(reference(6, 2), NAMESPACE_WIN32_AND_DOS, "x.tif")]),
    );
    // POSIX only.
    b.put(
        11,
        multi_name(&[(reference(6, 2), NAMESPACE_POSIX, "Case.TXT")]),
    );
    // POSIX before Win32: Win32 wins.
    b.put(
        12,
        multi_name(&[
            (reference(6, 2), NAMESPACE_POSIX, "posix"),
            (reference(6, 2), NAMESPACE_WIN32, "win32"),
        ]),
    );
    // DOS only: not reportable.
    b.put(
        13,
        multi_name(&[(reference(6, 2), NAMESPACE_DOS, "ONLY~1.DOS")]),
    );
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    let names: Vec<_> = records
        .iter()
        .map(|r| r.name.to_string_lossy().into_owned())
        .collect();
    for expected in ["long file name.tif", "x.tif", "Case.TXT", "win32"] {
        assert!(
            names.iter().any(|n| n == expected),
            "{expected} in {names:?}"
        );
    }
    for hidden in ["LONGFI~1.TIF", "posix", "ONLY~1.DOS"] {
        assert!(!names.iter().any(|n| n == hidden), "{hidden} leaked");
    }
    assert_eq!(stats.no_name, 1);
    assert_eq!(stats.lost(), 0, "no_name is reported but is not a loss");
}

#[test]
fn hard_links_report_the_first_name_and_count_the_file() {
    let mut b = tree();
    b.put(
        9,
        multi_name(&[
            (reference(6, 2), NAMESPACE_WIN32, "one.tif"),
            (ROOT, NAMESPACE_WIN32, "two.tif"),
            (ROOT, NAMESPACE_DOS, "TWO~1.TIF"),
        ]),
    );
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    let rec = by_name(&records, "one.tif");
    assert_eq!(rec.parent_frn, reference(6, 2));
    assert!(
        records
            .iter()
            .all(|r| r.name.to_string_lossy() != "two.tif")
    );
    assert_eq!(stats.hard_link_files, 1);
    assert!(stats.hard_link_frns.is_empty(), "listing is opt-in");
    let opts = MftScanOptions {
        list_hard_links: true,
        ..MftScanOptions::default()
    };
    let (_, stats) = scan_with(&img, &b, &opts).unwrap();
    assert_eq!(stats.hard_link_frns, [rec.frn]);
}

#[test]
fn unused_extension_and_unmarked_records_are_not_reported() {
    let mut b = tree();
    // Not in use (flag clear) but marked in the bitmap.
    let mut gone = simple_file(1024, 1, false, ROOT, "deleted.txt", 0x20);
    gone[0x16] = 0;
    seal_after_edit(&mut gone);
    b.put(9, gone);
    // An extension record of file 7 (base reference set).
    b.put(
        10,
        record(
            1024,
            1,
            RECORD_IN_USE,
            reference(7, 3),
            &[resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(ROOT, NAMESPACE_WIN32, "ext-name"),
            )],
        ),
    );
    // In use but cleared in the bitmap.
    b.put(11, simple_file(1024, 1, false, ROOT, "unmarked.txt", 0x20));
    b.unmark(11);
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    let names: Vec<_> = records
        .iter()
        .map(|r| r.name.to_string_lossy().into_owned())
        .collect();
    assert!(!names.contains(&"deleted.txt".to_owned()));
    assert!(!names.contains(&"ext-name".to_owned()));
    assert!(!names.contains(&"unmarked.txt".to_owned()));
    assert_eq!(stats.extension, 1);
    assert_eq!(stats.lost(), 0);
}

/// Re-applies the fixups after a fixture was edited post-sealing.
fn seal_after_edit(r: &mut [u8]) {
    // Undo (restore tails from the array), then seal again.
    apply_fixups(r).unwrap();
    seal(r);
}

#[test]
fn torn_and_damaged_records_are_counted_not_fatal() {
    let mut b = tree();
    let mut torn = simple_file(1024, 1, false, ROOT, "torn.txt", 0x20);
    torn[1022] ^= 0x55;
    b.put(9, torn);
    let mut baad = simple_file(1024, 1, false, ROOT, "baad.txt", 0x20);
    baad[..4].copy_from_slice(b"BAAD");
    b.put(10, baad);
    b.put(11, vec![0_u8; 1024]); // marked in use, but zeroed
    let mut bad_header = simple_file(1024, 1, false, ROOT, "badhdr.txt", 0x20);
    bad_header[0x18..0x1C].copy_from_slice(&5000_u32.to_le_bytes());
    b.put(12, bad_header);
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(records.len(), 5);
    assert_eq!((stats.torn, stats.not_file, stats.malformed), (1, 2, 1));
    assert_eq!(stats.lost(), 4);
    assert_eq!((stats.torn_retried, stats.torn_recovered), (1, 0));
}

#[test]
fn names_in_extension_records_are_found_through_the_attribute_list() {
    let mut b = tree();
    // Record 9: no $FILE_NAME of its own; the attribute list says they live in record 10.
    let mut list = attr_list_entry(ATTR_STANDARD_INFORMATION, 0, reference(9, 1));
    list.extend(attr_list_entry(ATTR_FILE_NAME, 0, reference(10, 1)));
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 0x20, 0)),
                resident_attr(ATTR_ATTRIBUTE_LIST, "", &list),
            ],
        ),
    );
    b.put(
        10,
        record(
            1024,
            1,
            RECORD_IN_USE,
            reference(9, 1),
            &[resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(reference(6, 2), NAMESPACE_WIN32, "spilled.gpkg"),
            )],
        ),
    );
    // A liar: claims to belong to another base record -> ignored.
    let mut list2 = attr_list_entry(ATTR_FILE_NAME, 0, reference(12, 1));
    list2.extend(attr_list_entry(ATTR_FILE_NAME, 0, reference(9999, 1))); // out of range
    b.put(
        11,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_ATTRIBUTE_LIST, "", &list2),
                resident_attr(
                    ATTR_FILE_NAME,
                    "",
                    &file_name_value(ROOT, NAMESPACE_WIN32, "own-name"),
                ),
            ],
        ),
    );
    b.put(
        12,
        record(
            1024,
            1,
            RECORD_IN_USE,
            reference(7, 3),
            &[resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(ROOT, NAMESPACE_WIN32, "stolen"),
            )],
        ),
    );
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    let spilled = by_name(&records, "spilled.gpkg");
    assert_eq!(spilled.frn, reference(9, 1));
    assert_eq!(spilled.parent_frn, reference(6, 2));
    assert_eq!(by_name(&records, "own-name").frn, reference(11, 1));
    assert!(records.iter().all(|r| r.name.to_string_lossy() != "stolen"));
    // The extension records come after their base files, so the sequential pass has seen them
    // by the time the held-back files are finished: no random reads.
    assert_eq!(stats.extension_reads, 0);
    assert_eq!(stats.extension_errors, 2);

    // With the remember-as-you-go cache disabled, each extension record is read on demand and
    // the result is the same.
    let no_cache = MftScanOptions {
        ext_cache_bytes: 0,
        ..MftScanOptions::default()
    };
    let (records2, stats2) = scan_with(&img, &b, &no_cache).unwrap();
    assert_eq!(stats2.extension_reads, 2);
    assert_eq!(stats2.extension_errors, 2);
    let mut a: Vec<_> = records
        .iter()
        .map(|r| (r.frn, r.parent_frn, r.name.clone()))
        .collect();
    let mut c: Vec<_> = records2
        .iter()
        .map(|r| (r.frn, r.parent_frn, r.name.clone()))
        .collect();
    a.sort_by_key(|r| r.0);
    c.sort_by_key(|r| r.0);
    assert_eq!(a, c);
}

#[test]
fn an_extension_record_before_its_base_is_used_without_deferring() {
    let mut b = tree();
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            reference(10, 1),
            &[resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(reference(6, 2), NAMESPACE_WIN32, "early.txt"),
            )],
        ),
    );
    let list = attr_list_entry(ATTR_FILE_NAME, 0, reference(9, 1));
    b.put(
        10,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 0x20, 0)),
                resident_attr(ATTR_ATTRIBUTE_LIST, "", &list),
            ],
        ),
    );
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(by_name(&records, "early.txt").frn, reference(10, 1));
    assert_eq!(stats.extension_reads, 0);
    assert_eq!(stats.extension_errors, 0);
}

#[test]
fn skip_reserved_also_hides_the_children_of_extend() {
    let mut b = ImageBuilder::new(4096, 1024, 64, 8);
    b.put(5, simple_file(1024, 5, true, ROOT, ".", 0x10));
    b.put(
        20,
        simple_file(1024, 1, false, reference(11, 1), "$ObjId", 0x20),
    );
    b.put(21, simple_file(1024, 1, false, ROOT, "keep.txt", 0x20));
    b.put(
        22,
        simple_file(1024, 1, true, reference(11, 1), "$RmMetadata", 0x10),
    );
    b.put(
        23,
        simple_file(1024, 1, false, reference(22, 1), "$Repair", 0x20),
    );
    b.put(
        24,
        simple_file(1024, 1, false, reference(22, 1), "$TxfLog", 0x20),
    );
    let img = b.build();
    let opts = MftScanOptions {
        skip_reserved: true,
        ..MftScanOptions::default()
    };
    let (records, stats) = scan_with(&img, &b, &opts).unwrap();
    let names: Vec<_> = records
        .iter()
        .map(|r| r.name.to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [".", "keep.txt", "$RmMetadata", "$TxfLog"]);
    assert_eq!(stats.lost(), 0);
}

#[test]
fn only_win32_attribute_bits_are_reported() {
    let mut b = tree();
    // NTFS-internal bits (directory-index flag 0x1000_0000, 0x8000_0000, 0x4_0000) are dropped.
    b.put(
        9,
        simple_file(1024, 1, false, ROOT, "bits.txt", 0x9004_0027),
    );
    let img = b.build();
    let (records, _) = scan_image(&img, &b).unwrap();
    assert_eq!(by_name(&records, "bits.txt").file_attributes, 0x27);
}

#[test]
fn a_non_resident_attribute_list_is_read_from_disk() {
    let mut b = tree();
    let mut list = attr_list_entry(ATTR_FILE_NAME, 0, reference(10, 1));
    list.resize(5000, 0);
    // Put the list in cluster 20 (outside the MFT).
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 0x20, 0)),
                nonresident_attr(
                    ATTR_ATTRIBUTE_LIST,
                    "",
                    0,
                    5000,
                    5000,
                    &encode_runs(&[(2, Some(20))]),
                ),
            ],
        ),
    );
    b.put(
        10,
        record(
            1024,
            1,
            RECORD_IN_USE,
            reference(9, 1),
            &[resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(ROOT, NAMESPACE_WIN32, "far.txt"),
            )],
        ),
    );
    let mut img = b.build();
    img[20 * 4096..20 * 4096 + list.len()].copy_from_slice(&list);
    let (records, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(by_name(&records, "far.txt").frn, reference(9, 1));
    assert_eq!(stats.extension_errors, 0);
}

#[test]
fn a_fragmented_mft_with_records_straddling_extents_scans_completely() {
    // 512-byte clusters and 1024-byte records: an extent boundary at an odd cluster splits a
    // record across two extents. Extents are far apart on the "disk".
    let mut b = ImageBuilder::new(512, 1024, 400, 0);
    b.extents = vec![(10, 3), (100, 5), (200, 4), (300, 8)]; // 20 clusters = 10 records
    b.bitmap_lcn = Some(50);
    for n in 5..10_u64 {
        b.put(n, simple_file(1024, 1, false, ROOT, &format!("f{n}"), 0x20));
    }
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(records.len(), 6, "{records:?}"); // $MFT + 5
    for n in 5..10_u64 {
        assert_eq!(by_name(&records, &format!("f{n}")).frn, reference(n, 1));
    }
    assert_eq!(stats.lost(), 0);
}

#[test]
fn a_fragmented_mft_described_through_an_attribute_list_scans_completely() {
    let mut b = ImageBuilder::new(1024, 1024, 400, 0);
    b.extents = vec![(10, 4), (100, 4), (200, 4), (300, 4)]; // 16 records
    b.bitmap_lcn = Some(50);
    b.extents_in_record0 = Some(2); // 8 records via record 0
    b.data_extension_record = 3; // the rest through extension record 3 (inside the first part)
    for n in 5..16_u64 {
        if n != 3 {
            b.put(n, simple_file(1024, 1, false, ROOT, &format!("f{n}"), 0x20));
        }
    }
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    // $MFT, f5..f15 (11 files); the extension record is not a file.
    assert_eq!(records.len(), 12, "{records:?}");
    assert_eq!(by_name(&records, "f15").frn, reference(15, 1));
    assert_eq!(stats.extension, 1);
}

#[test]
fn free_chunks_are_skipped_using_the_bitmap() {
    // 512 records (512 KiB); chunk size clamps to 64 KiB = 64 records. Records live only in the
    // first and last chunk.
    let mut b = ImageBuilder::new(4096, 1024, 400, 128);
    b.put(5, simple_file(1024, 1, true, ROOT, ".", 0x10));
    b.put(500, simple_file(1024, 1, false, ROOT, "last.txt", 0x20));
    let img = b.build();
    let opts = MftScanOptions {
        chunk_bytes: 1,
        ..MftScanOptions::default()
    };
    let (records, stats) = scan_with(&img, &b, &opts).unwrap();
    assert_eq!(records.len(), 3); // $MFT, root, last.txt
    assert_eq!(by_name(&records, "last.txt").frn, reference(500, 1));
    assert_eq!(stats.chunks_skipped, 6);
    assert_eq!(stats.reads, 2);
    assert_eq!(stats.bytes, 2 * 64 * 1024);
}

#[test]
fn a_missing_bitmap_falls_back_to_the_header_flag() {
    let mut b = tree();
    b.bitmap_lcn = None;
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(records.len(), 5);
    assert_eq!(stats.not_file, 11); // zeroed slots
    assert_eq!(stats.lost(), 0);
}

#[test]
fn the_sink_can_stop_the_scan() {
    let b = tree();
    let img = b.build();
    let scanner = MftScanner::open(&img[..], b.geometry()).unwrap();
    let mut seen = 0;
    let stats = scanner
        .scan(&MftScanOptions::default(), &mut |_| {
            seen += 1;
            if seen == 2 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
    assert_eq!(seen, 2);
    assert!(stats.cancelled);
}

// ---- hostile $MFT ----------------------------------------------------------------------------

#[test]
fn hostile_mft_records_are_rejected_at_open() {
    let b = tree();
    let good = b.build();
    let at = b.geometry().mft_start_byte() as usize;

    // Not a FILE record / bad fixups / bad flags.
    for what in 0..3 {
        let mut img = good.clone();
        match what {
            0 => img[at..at + 1024].fill(0),
            1 => img[at + 1022] ^= 0xFF,
            _ => img[at + 0x16] = 0,
        }
        assert!(
            MftScanner::open(&img[..], b.geometry()).is_err(),
            "case {what}"
        );
    }

    // Truncated image: reading record 0 fails cleanly.
    assert!(matches!(
        MftScanner::open(&good[..at + 100], b.geometry()),
        Err(MftError::Io(_))
    ));
}

fn record0_with(b: &ImageBuilder, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut b = b.clone();
    b.put(0, record(1024, 1, RECORD_IN_USE, 0, attrs));
    b.build()
}

#[test]
fn lying_mft_data_attributes_are_bounded() {
    let b = tree();
    let geom = b.geometry();
    let name = resident_attr(
        ATTR_FILE_NAME,
        "",
        &file_name_value(ROOT, NAMESPACE_WIN32, "$MFT"),
    );
    let one = encode_runs(&[(4, Some(4))]);
    // Declared size far beyond the runs: the table is truncated, which is an error (not a
    // silently shorter scan), and nothing is read outside the runs.
    let img = record0_with(
        &b,
        &[
            name.clone(),
            nonresident_attr(ATTR_DATA, "", 0, u64::MAX >> 1, u64::MAX >> 1, &one),
        ],
    );
    assert!(MftScanner::open(&img[..], geom).is_err());
    // Initialized size below the real size: allowed, only the initialized part is scanned and
    // the rest is reported.
    let img = record0_with(
        &b,
        &[
            name.clone(),
            nonresident_attr(ATTR_DATA, "", 0, 16384, 8192, &one),
        ],
    );
    let scanner = MftScanner::open(&img[..], geom).unwrap();
    assert_eq!(scanner.record_count(), 8);
    let stats = scanner
        .scan(&MftScanOptions::default(), &mut |_| {
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(stats.uninitialized_slots, 8);

    // Resident $DATA, missing $DATA, $DATA not starting at VCN 0, size zero.
    for (what, attrs) in [
        (
            "resident",
            vec![name.clone(), resident_attr(ATTR_DATA, "", &[1, 2, 3])],
        ),
        ("missing", vec![name.clone()]),
        (
            "start vcn",
            vec![
                name.clone(),
                nonresident_attr(ATTR_DATA, "", 4, 16384, 16384, &one),
            ],
        ),
        (
            "empty",
            vec![name.clone(), nonresident_attr(ATTR_DATA, "", 0, 0, 0, &one)],
        ),
        (
            "tiny",
            vec![
                name.clone(),
                nonresident_attr(ATTR_DATA, "", 0, 1000, 1000, &one),
            ],
        ),
        (
            "run outside the volume",
            vec![
                name.clone(),
                nonresident_attr(
                    ATTR_DATA,
                    "",
                    0,
                    16384,
                    16384,
                    &encode_runs(&[(4, Some(62))]),
                ),
            ],
        ),
        (
            "negative lcn",
            vec![
                name.clone(),
                nonresident_attr(ATTR_DATA, "", 0, 16384, 16384, &[0x11, 0x04, 0xF0, 0]),
            ],
        ),
    ] {
        let img = record0_with(&b, &attrs);
        assert!(MftScanner::open(&img[..], geom).is_err(), "{what}");
    }
}

#[test]
fn a_huge_record_count_is_refused() {
    // 2^33 bytes of sparse $MFT would be 8M records of 1 KiB, fine; 2^43 would exceed the cap.
    let b = tree();
    let name = resident_attr(
        ATTR_FILE_NAME,
        "",
        &file_name_value(ROOT, NAMESPACE_WIN32, "$MFT"),
    );
    let mut g = b.geometry();
    g.total_clusters = 1 << 40; // so the sparse run is "inside the volume"
    let runs = encode_runs(&[(1 << 36, None)]);
    let size = (1_u64 << 36) * 4096;
    let mut img = record0_with(
        &b,
        &[name, nonresident_attr(ATTR_DATA, "", 0, size, size, &runs)],
    );
    // The image is only 64 clusters long, but open() must not allocate from the claim.
    img.truncate(64 * 4096);
    match MftScanner::open(&img[..], g) {
        Err(MftError::Parse(MftParseError::Limit { .. })) => {}
        other => panic!("expected the record cap, got {other:?}"),
    }
}

#[test]
fn mft_attribute_list_loops_and_gaps_are_rejected() {
    let mut b = ImageBuilder::new(1024, 1024, 400, 0);
    b.extents = vec![(10, 4), (100, 4)];
    b.bitmap_lcn = Some(50);
    b.extents_in_record0 = Some(1);
    b.data_extension_record = 1;
    let img = b.build();
    assert!(MftScanner::open(&img[..], b.geometry()).is_ok());

    // The continuation lies outside the extents known so far.
    let mut bad = b.clone();
    bad.data_extension_record = 7; // VCN 7 is in the not-yet-described second extent
    let img = bad.build();
    assert!(MftScanner::open(&img[..], bad.geometry()).is_err());
}

// ---- mutation / truncation fuzzing -----------------------------------------------------------

#[test]
fn mutated_images_never_panic() {
    let mut b = tree();
    b.put(
        9,
        multi_name(&[(reference(6, 2), 1, "a"), (ROOT, 1, "b"), (ROOT, 2, "B~1")]),
    );
    let mut list = attr_list_entry(ATTR_FILE_NAME, 0, reference(11, 1));
    list.extend(attr_list_entry(ATTR_DATA, 0, reference(9, 1)));
    b.put(
        10,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 0x20, 0)),
                resident_attr(ATTR_ATTRIBUTE_LIST, "", &list),
            ],
        ),
    );
    b.put(
        11,
        record(
            1024,
            1,
            RECORD_IN_USE,
            reference(10, 1),
            &[resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(ROOT, 1, "ext"),
            )],
        ),
    );
    let base = b.build();
    let geom = b.geometry();
    let mft_start = geom.mft_start_byte() as usize;
    let mft_end = mft_start + 4 * 4096;
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let mut opened = 0;
    for _ in 0..6_000 {
        let mut img = base.clone();
        for _ in 0..1 + rng.next() % 8 {
            // Concentrate on the $MFT area and the bitmap cluster, sometimes anywhere.
            let at = if rng.next() & 3 == 0 {
                rng.next() as usize % img.len()
            } else if rng.next() & 1 == 0 {
                mft_start + rng.next() as usize % (mft_end - mft_start)
            } else {
                2 * 4096 + rng.next() as usize % 64
            };
            img[at] = rng.next() as u8;
        }
        if let Ok(scanner) = MftScanner::open(&img[..], geom) {
            opened += 1;
            let _ = scanner.scan(&MftScanOptions::default(), &mut |_| {
                ControlFlow::Continue(())
            });
        }
    }
    assert!(opened > 1000, "fuzzing never got past open(): {opened}");
}

#[test]
fn truncated_images_never_panic() {
    let b = tree();
    let img = b.build();
    let geom = b.geometry();
    let start = geom.mft_start_byte() as usize;
    for len in (0..img.len())
        .step_by(97)
        .chain([start, start + 1024, start + 4096, start + 8192])
    {
        if let Ok(scanner) = MftScanner::open(&img[..len], geom) {
            let _ = scanner.scan(&MftScanOptions::default(), &mut |_| {
                ControlFlow::Continue(())
            });
        }
    }
}

#[test]
fn random_noise_records_never_panic() {
    let mut rng = XorShift(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..2_000 {
        let mut r = vec![0_u8; 1024];
        for b in &mut r {
            *b = rng.next() as u8;
        }
        // Give some of them a plausible shell so the parser goes deeper.
        if rng.next() & 1 == 0 {
            r[..4].copy_from_slice(b"FILE");
            r[4..6].copy_from_slice(&0x30_u16.to_le_bytes());
            r[6..8].copy_from_slice(&3_u16.to_le_bytes());
            r[0x14..0x16].copy_from_slice(&0x38_u16.to_le_bytes());
            seal(&mut r);
        }
        if apply_fixups(&mut r).is_ok()
            && let Ok(h) = parse_record_header(&r)
        {
            for a in Attrs::new(&r, &h).flatten() {
                if let AttrBody::NonResident { runlist, .. } = a.body {
                    let _ = decode_runlist(runlist, 0, 1 << 30, &mut Vec::new());
                }
                if let AttrBody::Resident { value } = a.body {
                    let _ = parse_file_name(value);
                    let _ = parse_standard_info(value);
                    let _ = AttrListEntries::new(value).count();
                }
            }
        }
    }
}

/// A byte source that tears one record in the first read that covers it (a write in flight),
/// and serves it whole afterwards.
struct TearOnce<'a> {
    img: &'a [u8],
    at: u64,
    torn: std::cell::Cell<bool>,
}

impl crate::mft_scan::ByteSource for TearOnce<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.img.read_at(offset, buf)?;
        let end = offset + buf.len() as u64;
        if !self.torn.get() && offset <= self.at && self.at < end && buf.len() > 1024 {
            buf[(self.at - offset) as usize] ^= 0xFF;
            self.torn.set(true);
        }
        Ok(())
    }
}

#[test]
fn a_torn_record_is_read_again_and_recovered() {
    let b = tree();
    let img = b.build();
    let geom = b.geometry();
    // Record 7 lives at MFT start + 7 KiB; tear its last sector's tail.
    let at = geom.mft_start_byte() + 7 * 1024 + 1022;
    let src = TearOnce {
        img: &img,
        at,
        torn: std::cell::Cell::new(false),
    };
    let scanner = MftScanner::open(&src, geom).unwrap();
    let mut names = Vec::new();
    let stats = scanner
        .scan(&MftScanOptions::default(), &mut |r| {
            names.push(r.name.to_string_lossy().into_owned());
            ControlFlow::Continue(())
        })
        .unwrap();
    assert!(src.torn.get());
    assert!(names.contains(&"a.shp".to_owned()), "{names:?}");
    assert_eq!(
        (stats.torn, stats.torn_retried, stats.torn_recovered),
        (0, 1, 1)
    );
    assert_eq!(stats.lost(), 0);
}

#[test]
fn every_kind_of_loss_is_counted_in_lost() {
    // Bitmap-marked slot that is not a FILE record.
    let mut b = tree();
    b.put(9, vec![0_u8; 1024]);
    let img = b.build();
    let (_, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(
        (stats.not_file, stats.not_file_in_use, stats.lost()),
        (1, 1, 1)
    );

    // Without a bitmap the same zeroed slots are unknowns, not losses.
    let mut b = tree();
    b.bitmap_lcn = None;
    let img = b.build();
    let (_, stats) = scan_image(&img, &b).unwrap();
    assert!(stats.not_file > 0);
    assert_eq!(stats.lost(), 0);

    // Malformed $STANDARD_INFORMATION: the record is kept but the damage is counted.
    let mut b = tree();
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &[1, 2, 3]),
                resident_attr(
                    ATTR_FILE_NAME,
                    "",
                    &file_name_value(ROOT, NAMESPACE_WIN32, "short-si.txt"),
                ),
            ],
        ),
    );
    let img = b.build();
    let (records, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(by_name(&records, "short-si.txt").file_attributes, 0x80);
    assert_eq!((stats.malformed, stats.lost()), (1, 1));

    // Malformed $FILE_NAME (declared name longer than the value).
    let mut b = tree();
    let mut bad = file_name_value(ROOT, NAMESPACE_WIN32, "x");
    bad[0x40] = 200;
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[resident_attr(ATTR_FILE_NAME, "", &bad)],
        ),
    );
    let img = b.build();
    let (_, stats) = scan_image(&img, &b).unwrap();
    assert_eq!((stats.malformed, stats.no_name), (1, 1));
    assert!(stats.lost() >= 1);

    // A name list entry pointing outside the table.
    let mut b = tree();
    let list = attr_list_entry(ATTR_FILE_NAME, 0, reference(5000, 1));
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 0x20, 0)),
                resident_attr(ATTR_ATTRIBUTE_LIST, "", &list),
            ],
        ),
    );
    let img = b.build();
    let (_, stats) = scan_image(&img, &b).unwrap();
    assert_eq!(stats.extension_errors, 1);
    assert!(stats.lost() >= 1);

    // A non-resident list whose runs fall short of its size (second extent missing).
    let mut b = tree();
    b.put(
        9,
        record(
            1024,
            1,
            RECORD_IN_USE,
            0,
            &[
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 0x20, 0)),
                nonresident_attr(
                    ATTR_ATTRIBUTE_LIST,
                    "",
                    0,
                    1 << 20,
                    1 << 20,
                    &encode_runs(&[(1, Some(20))]),
                ),
            ],
        ),
    );
    let img = b.build();
    let (_, stats) = scan_image(&img, &b).unwrap();
    assert!(stats.extension_errors >= 1, "{stats:?}");
}
