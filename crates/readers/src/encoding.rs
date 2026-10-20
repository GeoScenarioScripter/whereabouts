//! Text encodings of DBF tables: `.cpg` sidecars and dBase language driver IDs (LDID).
//!
//! Resolution follows GDAL's shapefile driver: a recognised `.cpg` wins over the LDID byte in the
//! DBF header. Code pages map to [`encoding_rs`] encodings; DOS code pages that the Encoding
//! Standard lacks (437, 850, 852, …) resolve to `None`. Undeclared or unsupported text decodes
//! as UTF-8 when valid, otherwise as Windows-1252 (the same fallback `whereabouts-crs` uses, D-32).

use encoding_rs::Encoding;

/// The encoding declared by a `.cpg` file's content (`"UTF-8"`, `"936"`, `"CP1251"`,
/// `"8859-5"`, `"ANSI 1252"`, an Encoding Standard label such as `"gbk"`, …).
pub(crate) fn from_cpg(text: &str) -> Option<&'static Encoding> {
    let label = text.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}');
    if label.is_empty() {
        return None;
    }
    let upper = label.to_ascii_uppercase();
    if upper == "UTF-8" || upper == "UTF8" {
        return Some(encoding_rs::UTF_8);
    }
    let iso = upper
        .strip_prefix("ISO")
        .map_or(upper.as_str(), |s| s.trim_start_matches(['-', '_', ' ']));
    if let Some(part) = iso.strip_prefix("8859") {
        return part
            .trim_start_matches(['-', '_'])
            .parse()
            .ok()
            .and_then(iso_8859);
    }
    let number = ["CP", "ANSI", "OEM", "WINDOWS", "MS"]
        .iter()
        .find_map(|p| upper.strip_prefix(p))
        .unwrap_or(&upper)
        .trim_start_matches([' ', '-', '_']);
    if let Ok(code_page) = number.parse::<u32>() {
        return from_code_page(code_page);
    }
    Encoding::for_label(label.as_bytes())
}

/// The encoding signalled by a DBF language driver ID (header byte 29); `None` for 0 (not set),
/// unknown IDs and code pages without an Encoding Standard equivalent. Table from GDAL's
/// shapefile driver.
pub(crate) fn from_ldid(ldid: u8) -> Option<&'static Encoding> {
    let code_page = match ldid {
        // GDAL reports 0x57 as ISO-8859-1, which the Encoding Standard decodes as Windows-1252.
        87 => 1252,
        1 | 9 | 11 | 13 | 15 | 17 | 21 | 24 | 25 | 27 => 437,
        2 | 10 | 14 | 16 | 18 | 20 | 22 | 26 | 29 | 37 | 55 => 850,
        3 | 88 | 89 => 1252,
        4 => 10000,
        8 | 23 | 102 => 865,
        19 | 123 => 932,
        28 | 108 => 863,
        31 | 34 | 35 | 64 | 100 | 135 => 852,
        36 => 860,
        38 | 101 => 866,
        77 | 122 => 936,
        78 | 121 => 949,
        79 | 120 => 950,
        80 | 124 => 874,
        103 => 861,
        106 | 134 => 737,
        107 | 136 => 857,
        150 => 10007,
        151 => 10029,
        200 => 1250,
        201 => 1251,
        202 => 1254,
        203 => 1253,
        204 => 1257,
        _ => return None,
    };
    from_code_page(code_page)
}

/// The DBF's encoding: the `.cpg` declaration if recognised, else the LDID.
pub(crate) fn resolve(cpg: Option<&str>, ldid: u8) -> Option<&'static Encoding> {
    cpg.and_then(from_cpg).or_else(|| from_ldid(ldid))
}

/// Decodes `bytes` with `encoding`, or as UTF-8 / Windows-1252 when it is unknown. Never fails;
/// undecodable sequences become U+FFFD.
pub(crate) fn decode(bytes: &[u8], encoding: Option<&'static Encoding>) -> String {
    match encoding {
        Some(e) => e.decode_without_bom_handling(bytes).0.into_owned(),
        None => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(bytes)
                .0
                .into_owned(),
        },
    }
}

fn from_code_page(code_page: u32) -> Option<&'static Encoding> {
    use encoding_rs as e;
    Some(match code_page {
        866 => e::IBM866,
        874 => e::WINDOWS_874,
        932 => e::SHIFT_JIS,
        936 => e::GBK,
        949 => e::EUC_KR,
        950 => e::BIG5,
        1250 => e::WINDOWS_1250,
        1251 => e::WINDOWS_1251,
        1252 => e::WINDOWS_1252,
        1253 => e::WINDOWS_1253,
        1254 => e::WINDOWS_1254,
        1255 => e::WINDOWS_1255,
        1256 => e::WINDOWS_1256,
        1257 => e::WINDOWS_1257,
        1258 => e::WINDOWS_1258,
        10000 => e::MACINTOSH,
        10007 => e::X_MAC_CYRILLIC,
        20866 => e::KOI8_R,
        21866 => e::KOI8_U,
        28591..=28606 => return iso_8859(code_page - 28590),
        54936 => e::GB18030,
        65001 => e::UTF_8,
        _ => return None,
    })
}

fn iso_8859(part: u32) -> Option<&'static Encoding> {
    use encoding_rs as e;
    Some(match part {
        // The Encoding Standard decodes ISO-8859-1 labels as Windows-1252 (a superset for text).
        1 => e::WINDOWS_1252,
        2 => e::ISO_8859_2,
        3 => e::ISO_8859_3,
        4 => e::ISO_8859_4,
        5 => e::ISO_8859_5,
        6 => e::ISO_8859_6,
        7 => e::ISO_8859_7,
        8 => e::ISO_8859_8,
        9 => e::WINDOWS_1254,
        10 => e::ISO_8859_10,
        13 => e::ISO_8859_13,
        14 => e::ISO_8859_14,
        15 => e::ISO_8859_15,
        16 => e::ISO_8859_16,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpg_labels() {
        let cases: &[(&str, Option<&'static Encoding>)] = &[
            ("UTF-8", Some(encoding_rs::UTF_8)),
            ("utf8\r\n", Some(encoding_rs::UTF_8)),
            ("\u{feff}UTF-8\0", Some(encoding_rs::UTF_8)),
            ("65001", Some(encoding_rs::UTF_8)),
            ("CP936", Some(encoding_rs::GBK)),
            ("936", Some(encoding_rs::GBK)),
            ("GBK", Some(encoding_rs::GBK)),
            ("1251", Some(encoding_rs::WINDOWS_1251)),
            ("ANSI 1251", Some(encoding_rs::WINDOWS_1251)),
            ("windows-1251", Some(encoding_rs::WINDOWS_1251)),
            ("8859-5", Some(encoding_rs::ISO_8859_5)),
            ("88595", Some(encoding_rs::ISO_8859_5)),
            ("ISO-8859-1", Some(encoding_rs::WINDOWS_1252)),
            ("ISO88591", Some(encoding_rs::WINDOWS_1252)),
            ("big5", Some(encoding_rs::BIG5)),
            ("437", None),
            ("8859-99", None),
            ("", None),
            ("   ", None),
            ("not an encoding", None),
            ("99999999999999999999", None),
        ];
        for (label, expected) in cases {
            assert_eq!(from_cpg(label), *expected, "{label:?}");
        }
    }

    #[test]
    fn ldid_table() {
        assert_eq!(from_ldid(0), None);
        assert_eq!(from_ldid(87), Some(encoding_rs::WINDOWS_1252));
        assert_eq!(from_ldid(201), Some(encoding_rs::WINDOWS_1251));
        assert_eq!(from_ldid(77), Some(encoding_rs::GBK));
        assert_eq!(from_ldid(0x7A), Some(encoding_rs::GBK));
        assert_eq!(from_ldid(19), Some(encoding_rs::SHIFT_JIS));
        // DOS code page without an Encoding Standard equivalent.
        assert_eq!(from_ldid(1), None);
        assert_eq!(from_ldid(255), None);
    }

    #[test]
    fn cpg_wins_over_ldid_and_falls_back() {
        assert_eq!(resolve(Some("UTF-8"), 201), Some(encoding_rs::UTF_8));
        assert_eq!(
            resolve(Some("garbage"), 201),
            Some(encoding_rs::WINDOWS_1251)
        );
        assert_eq!(resolve(None, 77), Some(encoding_rs::GBK));
        assert_eq!(resolve(None, 0), None);
    }

    #[test]
    fn decoding() {
        // "中文" in GBK.
        assert_eq!(
            decode(&[0xD6, 0xD0, 0xCE, 0xC4], Some(encoding_rs::GBK)),
            "中文"
        );
        // "Имя" in Windows-1251.
        assert_eq!(
            decode(&[0xC8, 0xEC, 0xFF], Some(encoding_rs::WINDOWS_1251)),
            "Имя"
        );
        assert_eq!(decode("café".as_bytes(), None), "café");
        assert_eq!(decode(&[0x63, 0x61, 0x66, 0xE9], None), "café");
        assert_eq!(
            decode(&[0xFF, 0xFE], Some(encoding_rs::UTF_8)),
            "\u{FFFD}\u{FFFD}"
        );
    }
}
