//! Extraction status and timestamps.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Where a dataset's metadata extraction stands (catalog `datasets.status` + `status_detail`).
///
/// Serde: `"pending"`, `"ok"`, `"unsupported"`, `{"partial": "<reason>"}`, `{"error": "<reason>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionStatus {
    /// Grouped, not yet extracted (or dirty and queued for re-extraction).
    Pending,
    /// Extracted completely.
    Ok,
    /// Extracted with gaps or degraded facts; the reason says what.
    Partial(String),
    /// No native reader for this format (yet), or a name-only format.
    Unsupported,
    /// Extraction failed (malformed file, I/O error, …); the reason says why. Never a panic.
    Error(String),
}

impl ExtractionStatus {
    /// The status code: `pending` | `ok` | `partial` | `unsupported` | `error`.
    pub fn code(&self) -> &'static str {
        match self {
            ExtractionStatus::Pending => "pending",
            ExtractionStatus::Ok => "ok",
            ExtractionStatus::Partial(_) => "partial",
            ExtractionStatus::Unsupported => "unsupported",
            ExtractionStatus::Error(_) => "error",
        }
    }

    /// The reason for `Partial`/`Error` (catalog `status_detail`).
    pub fn detail(&self) -> Option<&str> {
        match self {
            ExtractionStatus::Partial(r) | ExtractionStatus::Error(r) => Some(r),
            _ => None,
        }
    }

    /// Rebuilds a status from [`Self::code`] and [`Self::detail`] (e.g. a catalog row). A missing
    /// detail for `partial`/`error` becomes an empty reason; a detail on other codes is an error.
    pub fn from_parts(code: &str, detail: Option<&str>) -> Result<Self, CoreError> {
        let reason = || detail.unwrap_or_default().to_owned();
        let status = match code {
            "pending" => ExtractionStatus::Pending,
            "ok" => ExtractionStatus::Ok,
            "unsupported" => ExtractionStatus::Unsupported,
            "partial" => return Ok(ExtractionStatus::Partial(reason())),
            "error" => return Ok(ExtractionStatus::Error(reason())),
            _ => {
                return Err(CoreError::UnknownCode {
                    kind: "ExtractionStatus",
                    code: code.to_owned(),
                });
            }
        };
        match detail {
            None => Ok(status),
            Some(d) => Err(CoreError::InvalidStatus(format!(
                "{code} takes no detail, got {d:?}"
            ))),
        }
    }
}

/// A point in time as nanoseconds since the Unix epoch (UTC). Covers 1677–2262.
///
/// Used for content mtimes (change detection) and extraction times. Nanosecond resolution keeps
/// NTFS's 100 ns timestamps exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Windows `FILETIME` ticks (100 ns) between 1601-01-01 and 1970-01-01.
    const FILETIME_UNIX_OFFSET: i128 = 116_444_736_000_000_000;

    /// Converts a `SystemTime`; `None` if outside the representable range.
    pub fn from_system_time(t: SystemTime) -> Option<Self> {
        let nanos: i128 = match t.duration_since(UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).ok()?,
            Err(e) => -i128::try_from(e.duration().as_nanos()).ok()?,
        };
        i64::try_from(nanos).ok().map(Timestamp)
    }

    /// Converts back to a `SystemTime`; `None` if the platform cannot represent it.
    pub fn to_system_time(self) -> Option<SystemTime> {
        let magnitude = Duration::from_nanos(self.0.unsigned_abs());
        if self.0 >= 0 {
            UNIX_EPOCH.checked_add(magnitude)
        } else {
            UNIX_EPOCH.checked_sub(magnitude)
        }
    }

    /// Converts a Windows `FILETIME` value (100 ns ticks since 1601-01-01 UTC), as found in USN
    /// records and NTFS attributes; `None` if outside the representable range.
    pub fn from_filetime(ticks: u64) -> Option<Self> {
        let nanos = (i128::from(ticks) - Self::FILETIME_UNIX_OFFSET) * 100;
        i64::try_from(nanos).ok().map(Timestamp)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn status_parts_round_trip() {
        let all = [
            ExtractionStatus::Pending,
            ExtractionStatus::Ok,
            ExtractionStatus::Partial("stale gpkg_contents bounds".into()),
            ExtractionStatus::Unsupported,
            ExtractionStatus::Error("truncated .shp header".into()),
        ];
        for s in &all {
            assert_eq!(
                ExtractionStatus::from_parts(s.code(), s.detail()).unwrap(),
                *s
            );
            let json = serde_json::to_string(s).unwrap();
            assert_eq!(serde_json::from_str::<ExtractionStatus>(&json).unwrap(), *s);
        }
        assert_eq!(serde_json::to_string(&all[0]).unwrap(), "\"pending\"");
        assert_eq!(
            serde_json::to_string(&all[4]).unwrap(),
            r#"{"error":"truncated .shp header"}"#
        );
        assert!(ExtractionStatus::from_parts("ok", Some("x")).is_err());
        assert!(ExtractionStatus::from_parts("done", None).is_err());
        assert_eq!(
            ExtractionStatus::from_parts("error", None).unwrap(),
            ExtractionStatus::Error(String::new())
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(
            Timestamp::from_filetime(116_444_736_000_000_000),
            Some(Timestamp(0))
        );
        assert_eq!(
            Timestamp::from_filetime(116_444_736_000_000_001),
            Some(Timestamp(100))
        );
        assert_eq!(Timestamp::from_filetime(0), None, "1601 is before 1677");
        assert_eq!(Timestamp::from_filetime(u64::MAX), None);
        let now = SystemTime::now();
        let t = Timestamp::from_system_time(now).unwrap();
        // Round trip is exact on platforms with <= 1 ns SystemTime resolution.
        assert_eq!(
            Timestamp::from_system_time(t.to_system_time().unwrap()),
            Some(t)
        );
        let before = Timestamp(-1_000_000_000);
        assert_eq!(
            Timestamp::from_system_time(before.to_system_time().unwrap()),
            Some(before)
        );
    }
}
