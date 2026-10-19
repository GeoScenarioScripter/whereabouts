//! Drive-letter parsing for volume arguments (pure, cross-platform).

use crate::error::Error;

/// Parses a volume argument into an upper-case drive letter.
///
/// Accepted spellings: `C`, `C:`, `C:\`, `C:/`, `\\.\C:`, `\\?\C:` and `\\.\C:\` (any case).
/// Mount points and `\\?\Volume{GUID}\` paths are not supported yet.
///
/// # Errors
/// [`Error::InvalidVolume`] for anything else.
pub fn parse_drive_letter(volume: &str) -> Result<char, Error> {
    let trimmed = volume.trim();
    let rest = trimmed
        .strip_prefix(r"\\.\")
        .or_else(|| trimmed.strip_prefix(r"\\?\"))
        .unwrap_or(trimmed);
    let rest = rest
        .strip_suffix('\\')
        .or_else(|| rest.strip_suffix('/'))
        .unwrap_or(rest);
    let rest = rest.strip_suffix(':').unwrap_or(rest);
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase()),
        _ => Err(Error::InvalidVolume(volume.to_owned())),
    }
}

/// `\\.\C:` — the device path used to open the raw volume.
#[must_use]
pub fn device_path(letter: char) -> String {
    format!(r"\\.\{letter}:")
}

/// `C:\` — the root directory path (for `GetVolumeInformationW` and the root FRN).
#[must_use]
pub fn root_path(letter: char) -> String {
    format!(r"{letter}:\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_common_spellings() {
        for s in [
            "C", "c", "C:", "c:", r"C:\", "C:/", r"\\.\C:", r"\\?\c:", r"\\.\C:\", " D: ",
        ] {
            let expected = if s.trim().to_ascii_uppercase().contains('D') {
                'D'
            } else {
                'C'
            };
            assert_eq!(parse_drive_letter(s).ok(), Some(expected), "{s:?}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for s in [
            "",
            ":",
            "CC:",
            "1:",
            r"C:\data",
            r"\\server\share",
            r"\\?\Volume{0b5c2d55-0000-0000-0000-100000000000}\",
            "é:",
        ] {
            assert!(
                matches!(parse_drive_letter(s), Err(Error::InvalidVolume(_))),
                "{s:?}"
            );
        }
    }

    #[test]
    fn paths() {
        assert_eq!(device_path('C'), r"\\.\C:");
        assert_eq!(root_path('C'), r"C:\");
    }
}
