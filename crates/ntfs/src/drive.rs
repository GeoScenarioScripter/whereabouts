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

/// `GetDriveTypeW` results (`winbase.h`).
pub mod drive_type {
    /// `DRIVE_UNKNOWN`.
    pub const UNKNOWN: u32 = 0;
    /// `DRIVE_NO_ROOT_DIR`: nothing mounted at this letter.
    pub const NO_ROOT_DIR: u32 = 1;
    /// `DRIVE_REMOVABLE`: USB stick, SD card.
    pub const REMOVABLE: u32 = 2;
    /// `DRIVE_FIXED`: internal or external disk.
    pub const FIXED: u32 = 3;
    /// `DRIVE_REMOTE`: mapped network drive.
    pub const REMOTE: u32 = 4;
    /// `DRIVE_CDROM`.
    pub const CDROM: u32 = 5;
    /// `DRIVE_RAMDISK`.
    pub const RAMDISK: u32 = 6;
}

/// Decides whether a drive letter is a local disk volume we may index (D-03: local files only).
///
/// `drive_type` is the `GetDriveTypeW` result for `X:\`; `dos_target` is the first target
/// `QueryDosDeviceW("X:")` reports (`\Device\HarddiskVolume3` for a real volume,
/// `\??\C:\some\dir` for a `SUBST` alias, `\Device\LanmanRedirector\…` or `\Device\Mup\…` for
/// network mappings). Accepts only `DRIVE_FIXED`/`DRIVE_REMOVABLE` letters that map to a
/// `\Device\` object other than a network redirector.
///
/// # Errors
/// A short human-readable reason for [`Error::NotLocal`].
pub fn check_local(drive_type: u32, dos_target: &str) -> Result<(), &'static str> {
    match drive_type {
        drive_type::FIXED | drive_type::REMOVABLE => {}
        drive_type::REMOTE => return Err("network drive"),
        drive_type::CDROM => return Err("optical drive"),
        drive_type::RAMDISK => return Err("RAM disk"),
        drive_type::NO_ROOT_DIR => return Err("nothing mounted at this drive letter"),
        _ => return Err("unknown drive type"),
    }
    let target = dos_target.to_ascii_lowercase();
    if target.starts_with(r"\??\") {
        return Err("SUBST alias of a folder");
    }
    if !target.starts_with(r"\device\") {
        return Err("not backed by a volume device");
    }
    if [
        "\\device\\lanmanredirector",
        "\\device\\mup",
        "\\device\\webdavredirector",
    ]
    .iter()
    .any(|p| target.starts_with(p))
    {
        return Err("network redirector");
    }
    Ok(())
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
    fn local_volumes_are_accepted() {
        assert_eq!(
            check_local(drive_type::FIXED, r"\Device\HarddiskVolume3"),
            Ok(())
        );
        assert_eq!(
            check_local(drive_type::REMOVABLE, r"\Device\HarddiskVolume12"),
            Ok(())
        );
        // VHDs, Storage Spaces, BitLocker volumes are still `\Device\…` objects.
        assert_eq!(
            check_local(drive_type::FIXED, r"\Device\VhdHardDisk{1234}"),
            Ok(())
        );
    }

    #[test]
    fn non_local_drives_are_rejected() {
        let cases = [
            (
                drive_type::REMOTE,
                r"\Device\LanmanRedirector\;Z:0000000000012345\server\share",
            ),
            (drive_type::FIXED, r"\??\C:\data\gis"), // SUBST reports the underlying type
            (drive_type::FIXED, r"\Device\Mup\;WebDavRedirector\server"),
            (drive_type::CDROM, r"\Device\CdRom0"),
            (drive_type::RAMDISK, r"\Device\Ramdisk"),
            (drive_type::NO_ROOT_DIR, ""),
            (drive_type::UNKNOWN, r"\Device\HarddiskVolume3"),
            (drive_type::FIXED, ""),
        ];
        for (kind, target) in cases {
            assert!(check_local(kind, target).is_err(), "{kind} {target:?}");
        }
    }

    #[test]
    fn paths() {
        assert_eq!(device_path('C'), r"\\.\C:");
        assert_eq!(root_path('C'), r"C:\");
    }
}
