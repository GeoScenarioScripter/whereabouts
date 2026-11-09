//! Datasets: the unit of search results (D-04), grouped from files.

use serde::{Deserialize, Serialize};

use crate::code::code_enum;
use crate::{CoreError, DatasetId, DatasetKind, Format, Frn, VolumeId};

code_enum! {
    /// The role a member file plays in its dataset (stored in `dataset_files.role`).
    pub enum MemberRole {
        /// The anchor: the file or folder that defines the dataset (`.shp`, `.gpkg`, `.tif`, the
        /// `.gdb` folder, …). Exactly one per dataset.
        Main => "main",
        /// Shapefile index (`.shx`).
        Shx => "shx",
        /// dBase attribute table (`.dbf`) of a shapefile.
        Dbf => "dbf",
        /// CRS definition (`.prj`), for shapefiles and world-file rasters.
        Prj => "prj",
        /// DBF code page (`.cpg`).
        Cpg => "cpg",
        /// ESRI spatial index (`.sbn`).
        Sbn => "sbn",
        /// ESRI spatial index (`.sbx`).
        Sbx => "sbx",
        /// QGIS/MapServer spatial index (`.qix`).
        Qix => "qix",
        /// ArcGIS metadata (`.shp.xml`, `.tif.xml`).
        XmlMetadata => "xml",
        /// QGIS metadata (`.qmd`).
        Qmd => "qmd",
        /// GDAL PAM sidecar (`.aux.xml`); may hold georeferencing and statistics.
        AuxXml => "aux_xml",
        /// External overviews (`.ovr`).
        Ovr => "ovr",
        /// External mask (`.msk`).
        Msk => "msk",
        /// World file (`.tfw`, `.tifw`, `.jgw`, `.pgw`, `.wld`, …).
        WorldFile => "world_file",
        /// A file inside a folder dataset (FGDB `.gdbtable`, …); never surfaced on its own.
        Internal => "internal",
        /// Any other recognised sidecar.
        Other => "other",
    }
}

/// One file (or folder) belonging to a dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Member {
    /// The member's FRN on the dataset's volume.
    pub frn: Frn,
    /// What the member is for.
    pub role: MemberRole,
}

/// A logical dataset: one or more files grouped by the dataset grouper.
///
/// Invariants (checked by [`Dataset::new`], [`Dataset::add_member`] and deserialisation):
/// - `members` contains exactly one [`MemberRole::Main`] member, and its FRN is `id.anchor`;
/// - no FRN appears twice in `members`.
///
/// A file may belong to several datasets (e.g. `a.prj` next to both `a.shp` and `a.tif`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DatasetRepr", into = "DatasetRepr")]
pub struct Dataset {
    id: DatasetId,
    kind: DatasetKind,
    format: Format,
    members: Vec<Member>,
}

impl Dataset {
    /// Creates a dataset, validating the member invariants.
    pub fn new(
        id: DatasetId,
        kind: DatasetKind,
        format: Format,
        members: Vec<Member>,
    ) -> Result<Self, CoreError> {
        let mains: Vec<_> = members
            .iter()
            .filter(|m| m.role == MemberRole::Main)
            .collect();
        match mains.as_slice() {
            [main] if main.frn == id.anchor => {}
            [main] => {
                return Err(CoreError::InvalidDataset(format!(
                    "main member {} is not the anchor {}",
                    main.frn, id.anchor
                )));
            }
            other => {
                return Err(CoreError::InvalidDataset(format!(
                    "expected exactly one main member, found {}",
                    other.len()
                )));
            }
        }
        // O(n log n): FGDB folders can have thousands of members.
        let mut frns: Vec<Frn> = members.iter().map(|m| m.frn).collect();
        frns.sort_unstable();
        if let Some(dup) = frns.windows(2).find_map(|w| match w {
            [a, b] if a == b => Some(*a),
            _ => None,
        }) {
            return Err(CoreError::InvalidDataset(format!("duplicate member {dup}")));
        }
        Ok(Dataset {
            id,
            kind,
            format,
            members,
        })
    }

    /// The dataset's stable identity.
    pub fn id(&self) -> DatasetId {
        self.id
    }

    /// The volume the dataset lives on (`id().volume`).
    pub fn volume(&self) -> VolumeId {
        self.id.volume
    }

    /// FRN of the anchor file or folder (`id().anchor`).
    pub fn anchor(&self) -> Frn {
        self.id.anchor
    }

    /// The dataset's kind.
    pub fn kind(&self) -> DatasetKind {
        self.kind
    }

    /// The dataset's format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// All members, including the anchor (role `Main`).
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// FRNs of the members with the given role.
    pub fn members_with(&self, role: MemberRole) -> impl Iterator<Item = Frn> + '_ {
        self.members
            .iter()
            .filter(move |m| m.role == role)
            .map(|m| m.frn)
    }

    /// `true` if `frn` is a member (any role). O(n).
    pub fn contains(&self, frn: Frn) -> bool {
        self.members.iter().any(|m| m.frn == frn)
    }

    /// Adds a sidecar. Fails for a second `Main` member or a duplicate FRN.
    ///
    /// Cost: O(n) per call (linear duplicate check; no side index, to keep `Dataset` plain data).
    /// Fine for incremental sidecar changes; build large datasets (FGDB folders with thousands of
    /// members) in one go with [`Dataset::new`], which is O(n log n).
    pub fn add_member(&mut self, member: Member) -> Result<(), CoreError> {
        if member.role == MemberRole::Main {
            return Err(CoreError::InvalidDataset(
                "a dataset has exactly one main member".into(),
            ));
        }
        if self.contains(member.frn) {
            return Err(CoreError::InvalidDataset(format!(
                "duplicate member {}",
                member.frn
            )));
        }
        self.members.push(member);
        Ok(())
    }

    /// Removes a sidecar by FRN and returns it. The anchor cannot be removed (delete the dataset
    /// instead): returns `None` for the anchor or a non-member.
    pub fn remove_member(&mut self, frn: Frn) -> Option<Member> {
        if frn == self.id.anchor {
            return None;
        }
        let pos = self.members.iter().position(|m| m.frn == frn)?;
        Some(self.members.remove(pos))
    }
}

#[derive(Serialize, Deserialize)]
struct DatasetRepr {
    id: DatasetId,
    kind: DatasetKind,
    format: Format,
    members: Vec<Member>,
}

impl TryFrom<DatasetRepr> for Dataset {
    type Error = CoreError;

    fn try_from(r: DatasetRepr) -> Result<Self, CoreError> {
        Dataset::new(r.id, r.kind, r.format, r.members)
    }
}

impl From<Dataset> for DatasetRepr {
    fn from(d: Dataset) -> Self {
        DatasetRepr {
            id: d.id,
            kind: d.kind,
            format: d.format,
            members: d.members,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn id() -> DatasetId {
        DatasetId::new(VolumeId(1), Frn(10))
    }

    fn shapefile() -> Dataset {
        Dataset::new(
            id(),
            DatasetKind::Vector,
            Format::Shapefile,
            vec![
                Member {
                    frn: Frn(10),
                    role: MemberRole::Main,
                },
                Member {
                    frn: Frn(11),
                    role: MemberRole::Shx,
                },
                Member {
                    frn: Frn(12),
                    role: MemberRole::Dbf,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn invariants_are_enforced() {
        let main = Member {
            frn: Frn(10),
            role: MemberRole::Main,
        };
        let shx = Member {
            frn: Frn(11),
            role: MemberRole::Shx,
        };
        let k = DatasetKind::Vector;
        let f = Format::Shapefile;
        assert!(Dataset::new(id(), k, f, vec![shx]).is_err(), "no main");
        assert!(
            Dataset::new(id(), k, f, vec![main, main]).is_err(),
            "two mains"
        );
        let wrong = Member {
            frn: Frn(99),
            role: MemberRole::Main,
        };
        assert!(
            Dataset::new(id(), k, f, vec![wrong]).is_err(),
            "main != anchor"
        );
        let dup = Member {
            frn: Frn(10),
            role: MemberRole::Prj,
        };
        assert!(
            Dataset::new(id(), k, f, vec![main, dup]).is_err(),
            "duplicate frn"
        );
        assert!(Dataset::new(id(), k, f, vec![main, shx]).is_ok());
    }

    #[test]
    fn large_folder_dataset_duplicate_detection() {
        let gdb = DatasetId::new(VolumeId(1), Frn(1));
        let mut members = vec![Member {
            frn: Frn(1),
            role: MemberRole::Main,
        }];
        members.extend((2..5_000).rev().map(|i| Member {
            frn: Frn(i),
            role: MemberRole::Internal,
        }));
        let ok = Dataset::new(
            gdb,
            DatasetKind::Container,
            Format::FileGeodatabase,
            members.clone(),
        );
        assert_eq!(ok.unwrap().members().len(), 4_999);
        members.push(Member {
            frn: Frn(2_500),
            role: MemberRole::Internal,
        });
        let err = Dataset::new(
            gdb,
            DatasetKind::Container,
            Format::FileGeodatabase,
            members,
        );
        assert_eq!(
            err.unwrap_err(),
            CoreError::InvalidDataset(format!("duplicate member {}", Frn(2_500)))
        );
    }

    #[test]
    fn member_edits() {
        let mut ds = shapefile();
        let prj = Member {
            frn: Frn(13),
            role: MemberRole::Prj,
        };
        ds.add_member(prj).unwrap();
        assert!(ds.add_member(prj).is_err());
        assert!(
            ds.add_member(Member {
                frn: Frn(14),
                role: MemberRole::Main
            })
            .is_err()
        );
        assert_eq!(
            ds.members_with(MemberRole::Prj).collect::<Vec<_>>(),
            vec![Frn(13)]
        );
        assert_eq!(ds.remove_member(Frn(10)), None, "anchor stays");
        assert_eq!(ds.remove_member(Frn(13)), Some(prj));
        assert!(!ds.contains(Frn(13)));
        assert_eq!(ds.volume(), VolumeId(1));
        assert_eq!(ds.anchor(), Frn(10));
    }

    #[test]
    fn serde_round_trip_and_validation() {
        let ds = shapefile();
        let json = serde_json::to_string(&ds).unwrap();
        assert_eq!(serde_json::from_str::<Dataset>(&json).unwrap(), ds);
        let bad = json.replace("\"main\"", "\"prj\"");
        assert!(serde_json::from_str::<Dataset>(&bad).is_err());
    }

    #[test]
    fn role_codes_round_trip() {
        for r in MemberRole::ALL {
            assert_eq!(r.code().parse::<MemberRole>().unwrap(), *r);
        }
    }
}
