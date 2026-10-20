//! Text encodings of DBF tables: `.cpg` sidecars and dBase language driver IDs (LDID).
//!
//! Resolution follows GDAL's shapefile driver: a recognised `.cpg` wins over the LDID byte in the
//! DBF header. Code pages map to [`encoding_rs`] encodings. DOS code pages that the Encoding
//! Standard lacks (437, 850, 852, …) can't be honoured. Undeclared or unsupported text decodes as
//! UTF-8 when valid, otherwise as Windows-1252 (the same fallback `whereabouts-crs` uses, D-32).
//! [`TableEncoding`] records which declarations could not be honoured, so callers can report
//! them when they matter (non-ASCII names).

use encoding_rs::Encoding;

/// The encoding a table's text is decoded with, and how it was arrived at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableEncoding {
    /// The declared encoding in use; `None` = nothing usable was declared (UTF-8 if valid,
    /// otherwise Windows-1252).
    pub(crate) encoding: Option<&'static Encoding>,
    /// What declared `encoding`, e.g. `.cpg "936"` or `LDID 0x57`.
    pub(crate) declared_by: Option<String>,
    /// Declarations that could not be honoured: an unrecognised `.cpg` label, an unknown LDID,
    /// or an LDID code page the Encoding Standard lacks.
    pub(crate) unsupported: Vec<String>,
}

impl TableEncoding {
    /// Human-readable name of the decoding actually used.
    pub(crate) fn used(&self) -> String {
        match self.encoding {
            Some(e) => e.name().to_owned(),
            None => "UTF-8 (Windows-1252 where not valid UTF-8)".to_owned(),
        }
    }
}

/// A `.cpg` label with whitespace, NULs and a BOM trimmed.
fn clean_label(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}')
}

/// The encoding declared by a `.cpg` file's content (`"UTF-8"`, `"936"`, `"CP1251"`,
/// `"8859-5"`, `"ANSI 1252"`, an Encoding Standard label such as `"gbk"`, …).
pub(crate) fn from_cpg(text: &str) -> Option<&'static Encoding> {
    let label = clean_label(text);
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

/// The Windows code page a DBF language driver ID (header byte 29) stands for; `None` for 0
/// (not set) and unknown IDs. Table from GDAL's shapefile driver.
pub(crate) fn ldid_code_page(ldid: u8) -> Option<u32> {
    Some(match ldid {
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
    })
}

/// The DBF's encoding: the `.cpg` declaration if recognised, else the LDID's code page if
/// supported, else none. Every declaration that was present but not usable is recorded.
pub(crate) fn resolve(cpg: Option<&str>, ldid: u8) -> TableEncoding {
    let mut unsupported = Vec::new();
    if let Some(label) = cpg.map(clean_label).filter(|l| !l.is_empty()) {
        match from_cpg(label) {
            Some(e) => {
                return TableEncoding {
                    encoding: Some(e),
                    declared_by: Some(format!(".cpg {label:?}")),
                    unsupported,
                };
            }
            None => unsupported.push(format!(".cpg label {label:?} is not recognised")),
        }
    }
    if ldid != 0 {
        match ldid_code_page(ldid).map(|cp| (cp, from_code_page(cp))) {
            Some((_, Some(e))) => {
                return TableEncoding {
                    encoding: Some(e),
                    declared_by: Some(format!("LDID 0x{ldid:02X}")),
                    unsupported,
                };
            }
            Some((cp, None)) => unsupported.push(format!(
                "LDID 0x{ldid:02X} declares code page {cp}, which is not supported"
            )),
            None => unsupported.push(format!("LDID 0x{ldid:02X} is unknown")),
        }
    }
    TableEncoding {
        encoding: None,
        declared_by: None,
        unsupported,
    }
}

/// Decodes `bytes` with `encoding`, or as UTF-8 / Windows-1252 when it is unknown. Never fails:
/// undecodable sequences become U+FFFD, and the flag says whether that happened (only possible
/// with a declared encoding; the fallback decodes every byte).
pub(crate) fn decode(bytes: &[u8], encoding: Option<&'static Encoding>) -> (String, bool) {
    match encoding {
        Some(e) => {
            let (text, had_errors) = e.decode_without_bom_handling(bytes);
            (text.into_owned(), had_errors)
        }
        None => match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_owned(), false),
            Err(_) => {
                let (text, had_errors) =
                    encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
                (text.into_owned(), had_errors)
            }
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
        assert_eq!(ldid_code_page(0), None);
        assert_eq!(ldid_code_page(87), Some(1252));
        assert_eq!(ldid_code_page(201), Some(1251));
        assert_eq!(ldid_code_page(0x7A), Some(936));
        assert_eq!(ldid_code_page(1), Some(437));
        assert_eq!(ldid_code_page(255), None);
        assert_eq!(resolve(None, 87).encoding, Some(encoding_rs::WINDOWS_1252));
        assert_eq!(resolve(None, 77).encoding, Some(encoding_rs::GBK));
        assert_eq!(resolve(None, 19).encoding, Some(encoding_rs::SHIFT_JIS));
    }

    #[test]
    fn cpg_wins_over_ldid_and_falls_back() {
        let r = resolve(Some("UTF-8\n"), 201);
        assert_eq!(r.encoding, Some(encoding_rs::UTF_8));
        assert_eq!(r.declared_by.as_deref(), Some(".cpg \"UTF-8\""));
        assert!(r.unsupported.is_empty());

        let r = resolve(Some("garbage"), 201);
        assert_eq!(r.encoding, Some(encoding_rs::WINDOWS_1251));
        assert_eq!(r.declared_by.as_deref(), Some("LDID 0xC9"));
        assert_eq!(r.unsupported, [".cpg label \"garbage\" is not recognised"]);

        // An empty .cpg is no declaration at all.
        let r = resolve(Some(" \r\n"), 0);
        assert_eq!(
            r,
            TableEncoding {
                encoding: None,
                declared_by: None,
                unsupported: Vec::new()
            }
        );
    }

    #[test]
    fn unsupported_declarations_are_recorded() {
        let r = resolve(None, 0x01);
        assert_eq!(r.encoding, None);
        assert_eq!(
            r.unsupported,
            ["LDID 0x01 declares code page 437, which is not supported"]
        );
        assert!(r.used().contains("UTF-8"));
        let r = resolve(Some("klingon"), 0x02);
        assert_eq!(r.unsupported.len(), 2);
        assert_eq!(resolve(None, 0xEE).unsupported, ["LDID 0xEE is unknown"]);
    }

    #[test]
    fn decoding_reports_replacements() {
        // "中文" in GBK.
        assert_eq!(
            decode(&[0xD6, 0xD0, 0xCE, 0xC4], Some(encoding_rs::GBK)),
            ("中文".to_owned(), false)
        );
        // "Имя" in Windows-1251.
        assert_eq!(
            decode(&[0xC8, 0xEC, 0xFF], Some(encoding_rs::WINDOWS_1251)),
            ("Имя".to_owned(), false)
        );
        assert_eq!(decode("café".as_bytes(), None), ("café".to_owned(), false));
        assert_eq!(
            decode(&[0x63, 0x61, 0x66, 0xE9], None),
            ("café".to_owned(), false)
        );
        // Bytes that are invalid in the declared encoding are replaced, and that is reported.
        assert_eq!(
            decode(&[0xFF, 0xFE], Some(encoding_rs::UTF_8)),
            ("\u{FFFD}\u{FFFD}".to_owned(), true)
        );
        assert!(decode(&[0x81], Some(encoding_rs::SHIFT_JIS)).1);
    }
}
