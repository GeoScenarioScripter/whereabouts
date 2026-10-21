//! Snapshot round trips and corruption detection.

#![allow(clippy::unwrap_used)]

use whereabouts_core::{FileAttributes, Frn};
use whereabouts_nameindex::{NameIndex, Query, SnapshotError, synth};
use whereabouts_source::Checkpoint;

fn sample(n: usize) -> NameIndex {
    let mut index = NameIndex::new(synth::ROOT);
    for r in synth::volume(n, 7) {
        index.insert(&r).unwrap();
    }
    // An orphan and a renamed entry, so unresolved parents and tombstones are covered.
    index
        .insert_wtf8(
            Frn::from_parts(27, 1).unwrap(),
            Frn::from_parts(11, 11).unwrap(),
            b"$TxfLog",
            FileAttributes(FileAttributes::HIDDEN),
        )
        .unwrap();
    let first = index.entries().nth(3).unwrap();
    let (frn, parent) = (index.frn(first).unwrap(), index.parent(first).unwrap());
    let parent_frn = index.frn(parent).unwrap();
    index
        .insert_wtf8(
            frn,
            parent_frn,
            "renamed é.shp".as_bytes(),
            FileAttributes::default(),
        )
        .unwrap();
    index
}

fn bytes(index: &NameIndex, cp: Option<&Checkpoint>) -> Vec<u8> {
    let mut buf = Vec::new();
    let n = index.write_snapshot(&mut buf, cp).unwrap();
    assert_eq!(n as usize, buf.len());
    buf
}

fn same(a: &NameIndex, b: &NameIndex) {
    assert_eq!(a.len(), b.len());
    for id in a.entries() {
        let frn = a.frn(id).unwrap();
        let other = b.lookup(frn).unwrap();
        assert_eq!(a.path(id), b.path(other));
        assert_eq!(a.attrs(id), b.attrs(other));
    }
    for q in [
        Query::substring("road"),
        Query::wildcard("*.shp"),
        Query::substring("\\v").in_path(true),
    ] {
        assert_eq!(a.search(&q).total, b.search(&q).total);
    }
}

#[test]
fn round_trip_with_checkpoint() {
    let index = sample(5_000);
    let cp = Checkpoint::new(2, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    let buf = bytes(&index, Some(&cp));
    let snap = NameIndex::read_snapshot(buf.as_slice()).unwrap();
    assert_eq!(snap.checkpoint, Some(cp));
    assert_eq!(snap.index.root_frn(), synth::ROOT);
    same(&index, &snap.index);
    assert_eq!(
        snap.index.stats().dead_slots,
        0,
        "loading yields a compact index"
    );
}

#[test]
fn round_trip_empty_and_via_file() {
    let empty = NameIndex::new(Frn(0x0005_0000_0000_0005));
    let snap = NameIndex::read_snapshot(bytes(&empty, None).as_slice()).unwrap();
    assert!(snap.index.is_empty());
    assert_eq!(snap.checkpoint, None);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.wbni");
    let index = sample(2_000);
    index.save_snapshot(&path, None).unwrap();
    index.save_snapshot(&path, None).unwrap(); // replaces atomically
    let loaded = NameIndex::load_snapshot(&path).unwrap();
    same(&index, &loaded.index);
    assert!(!dir.path().join("c.wbni.tmp").exists());
}

#[test]
fn every_truncation_is_detected() {
    let buf = bytes(&sample(40), Some(&Checkpoint::new(1, vec![9; 16]).unwrap()));
    for len in 0..buf.len() {
        let err = NameIndex::read_snapshot(&buf[..len]).unwrap_err();
        assert!(
            matches!(err, SnapshotError::Truncated | SnapshotError::BadMagic),
            "len {len}: {err:?}"
        );
    }
}

#[test]
fn every_single_byte_corruption_is_detected() {
    let buf = bytes(&sample(40), Some(&Checkpoint::new(1, vec![9; 16]).unwrap()));
    for pos in 0..buf.len() {
        for flip in [0x01_u8, 0x80, 0xFF] {
            let mut bad = buf.clone();
            bad[pos] ^= flip;
            assert!(
                NameIndex::read_snapshot(bad.as_slice()).is_err(),
                "flip {flip:#x} at {pos} went unnoticed"
            );
        }
    }
}

#[test]
fn specific_errors() {
    let buf = bytes(&sample(10), None);
    let mut extra = buf.clone();
    extra.push(0);
    assert!(matches!(
        NameIndex::read_snapshot(extra.as_slice()),
        Err(SnapshotError::TrailingBytes)
    ));
    let mut magic = buf.clone();
    magic[0] = b'X';
    assert!(matches!(
        NameIndex::read_snapshot(magic.as_slice()),
        Err(SnapshotError::BadMagic)
    ));
    let mut version = buf.clone();
    version[8] = 9;
    assert!(matches!(
        NameIndex::read_snapshot(version.as_slice()),
        Err(SnapshotError::UnsupportedVersion(9))
    ));
    let mut crc = buf.clone();
    let last = crc.len() - 1;
    crc[last] ^= 1;
    assert!(matches!(
        NameIndex::read_snapshot(crc.as_slice()),
        Err(SnapshotError::ChecksumMismatch { .. })
    ));
    // A header claiming 2^60 entries fails as truncated without allocating for them.
    let mut huge = buf;
    huge[32..40].copy_from_slice(&(1_u64 << 60).to_le_bytes());
    assert!(matches!(
        NameIndex::read_snapshot(huge.as_slice()),
        Err(SnapshotError::Truncated)
    ));
}

#[test]
fn structurally_invalid_content_with_valid_crc_is_rejected() {
    // Re-seal mutated bodies with a correct CRC: validation, not just the checksum, must catch them.
    fn reseal(mut body: Vec<u8>) -> Vec<u8> {
        let crc = crc32(&body);
        body.extend_from_slice(&crc.to_le_bytes());
        body
    }
    fn crc32(b: &[u8]) -> u32 {
        let mut c = !0_u32;
        for &x in b {
            c ^= u32::from(x);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    let mut index = NameIndex::new(synth::ROOT);
    index
        .insert_wtf8(Frn(0x100), synth::ROOT, b"ab", FileAttributes::default())
        .unwrap();
    let buf = bytes(&index, None);
    let body = buf[..buf.len() - 4].to_vec();
    assert!(NameIndex::read_snapshot(reseal(body.clone()).as_slice()).is_ok());

    let entry = 40; // first entry record
    let mut bad_name = body.clone();
    bad_name[entry + 22] = 0xFF;
    assert!(matches!(
        NameIndex::read_snapshot(reseal(bad_name).as_slice()),
        Err(SnapshotError::Corrupt(_))
    ));
    let mut root_dup = body.clone();
    root_dup[entry..entry + 8].copy_from_slice(&synth::ROOT.0.to_le_bytes());
    assert!(matches!(
        NameIndex::read_snapshot(reseal(root_dup).as_slice()),
        Err(SnapshotError::Corrupt(_))
    ));
    let mut long = body.clone();
    long[entry + 20..entry + 22].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(NameIndex::read_snapshot(reseal(long).as_slice()).is_err());
    let mut dup = body.clone();
    dup[32] = 2;
    dup.extend_from_slice(&body[entry..]);
    assert!(matches!(
        NameIndex::read_snapshot(reseal(dup).as_slice()),
        Err(SnapshotError::Corrupt(_))
    ));
    let mut cp = body;
    cp[28] = 0xFF;
    cp[29] = 0xFF;
    cp[30] = 0xFF;
    assert!(NameIndex::read_snapshot(reseal(cp).as_slice()).is_err());
}
