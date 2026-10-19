//! Identities: volumes, files, datasets, layers.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies one indexed volume (more generally: one registered `Source`) within a catalog.
///
/// Assigned by the catalog when a source is first registered (its `volumes` row id) and never reused
/// within that catalog. It is *not* the NTFS volume serial number: cloned disks and VHDs share
/// serials, and future non-NTFS sources have none. The source's own stable key (e.g. the volume GUID
/// path) is what the catalog matches to find an existing `VolumeId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeId(pub u32);

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// A 64-bit file reference number: a file's stable identity on its volume.
///
/// On NTFS this is the `FileReferenceNumber` of a USN record: the low 48 bits are the MFT record
/// number, the high 16 bits the record's sequence number (incremented when the record is reused,
/// so a recycled record never aliases a deleted file). It survives renames and moves within the
/// volume. Sources without native file IDs must assign stable synthetic 64-bit values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Frn(pub u64);

impl Frn {
    /// Mask of the MFT record-number part (low 48 bits).
    pub const RECORD_MASK: u64 = (1 << 48) - 1;

    /// Builds an FRN from an MFT record number and sequence number.
    ///
    /// Returns `None` if `record` does not fit in 48 bits.
    pub const fn from_parts(record: u64, sequence: u16) -> Option<Self> {
        if record > Self::RECORD_MASK {
            return None;
        }
        Some(Frn(((sequence as u64) << 48) | record))
    }

    /// The MFT record number (low 48 bits).
    pub const fn record_number(self) -> u64 {
        self.0 & Self::RECORD_MASK
    }

    /// The sequence number (high 16 bits).
    pub const fn sequence(self) -> u16 {
        (self.0 >> 48) as u16
    }
}

impl fmt::Display for Frn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}

/// Stable identity of a dataset: the volume plus the FRN of the dataset's **anchor**.
///
/// The anchor is the file or folder that defines the dataset (`x.shp`, `x.gpkg`, `x.tif`, the
/// `x.gdb` folder, …). Consequences of this derivation:
/// - renaming or moving the anchor within the volume keeps the ID (no re-extraction needed if the
///   content stamp is unchanged);
/// - adding/removing sidecars keeps the ID;
/// - a tool that deletes and recreates the anchor produces a new ID (old dataset deleted, new one
///   created), which is the correct outcome;
/// - a hard-linked anchor reachable under two names yields one ID (documented limitation).
///
/// The catalog stores it as `(volume_id, anchor_frn)` with a unique index; its integer `datasets.id`
/// is a catalog-private surrogate key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DatasetId {
    /// The volume the dataset lives on.
    pub volume: VolumeId,
    /// FRN of the anchor file or folder.
    pub anchor: Frn,
}

impl DatasetId {
    /// Creates a dataset ID from its volume and anchor FRN.
    pub const fn new(volume: VolumeId, anchor: Frn) -> Self {
        DatasetId { volume, anchor }
    }
}

impl fmt::Display for DatasetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.volume, self.anchor)
    }
}

/// Identity of a layer within a dataset: the dataset plus the layer's ordinal.
///
/// The ordinal is the layer's index in `DatasetMetadata::layers` as returned by the reader (readers
/// must list layers in a deterministic order, e.g. GeoPackage `gpkg_contents` sorted by table
/// name). Layers are replaced wholesale on re-extraction, so the ordinal is only stable while the
/// dataset's layer set is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId {
    /// The owning dataset.
    pub dataset: DatasetId,
    /// Zero-based index of the layer in the dataset's layer list.
    pub ordinal: u32,
}

impl LayerId {
    /// Creates a layer ID.
    pub const fn new(dataset: DatasetId, ordinal: u32) -> Self {
        LayerId { dataset, ordinal }
    }
}

impl fmt::Display for LayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.dataset, self.ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frn_parts_round_trip() {
        let frn = Frn::from_parts(5, 5).expect("fits");
        assert_eq!(frn.0, 0x0005_0000_0000_0005);
        assert_eq!(frn.record_number(), 5);
        assert_eq!(frn.sequence(), 5);
        assert_eq!(Frn::from_parts(1 << 48, 0), None);
        let max = Frn(u64::MAX);
        assert_eq!(max.record_number(), Frn::RECORD_MASK);
        assert_eq!(max.sequence(), u16::MAX);
    }

    #[test]
    fn display_forms() {
        let id = DatasetId::new(VolumeId(3), Frn(0x2a));
        assert_eq!(id.to_string(), "v3:0x000000000000002a");
        assert_eq!(LayerId::new(id, 1).to_string(), "v3:0x000000000000002a#1");
    }
}
