//! Conversions between core types and SQLite storage values.

use whereabouts_core::{CrsDefinition, Frn, VolumeId};

use crate::error::{CatalogError, Result, corrupt};

/// FRNs are full 64-bit values; SQLite integers are signed. Store the bit pattern.
pub(crate) fn frn_to_sql(frn: Frn) -> i64 {
    i64::from_ne_bytes(frn.0.to_ne_bytes())
}

/// Inverse of [`frn_to_sql`].
pub(crate) fn frn_from_sql(v: i64) -> Frn {
    Frn(u64::from_ne_bytes(v.to_ne_bytes()))
}

pub(crate) fn volume_from_sql(v: i64) -> Result<VolumeId> {
    u32::try_from(v)
        .map(VolumeId)
        .map_err(|_| corrupt(format!("volume id {v} out of range")))
}

/// A `u64` count or size → SQLite integer.
pub(crate) fn u64_to_sql(v: u64, what: &str) -> Result<i64> {
    i64::try_from(v).map_err(|_| CatalogError::OutOfRange(format!("{what} = {v}")))
}

/// A stored integer → `u64`; negative values mean the row is corrupt.
pub(crate) fn u64_from_sql(v: i64, what: &str) -> Result<u64> {
    u64::try_from(v).map_err(|_| corrupt(format!("{what} = {v}")))
}

pub(crate) fn u32_from_sql(v: i64, what: &str) -> Result<u32> {
    u32::try_from(v).map_err(|_| corrupt(format!("{what} = {v}")))
}

/// Non-finite floats can't be stored faithfully (SQLite turns NaN into NULL): keep finite ones.
pub(crate) fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// Stable 64-bit FNV-1a hash of a CRS definition, used to de-duplicate `crs` rows.
///
/// Not `std::hash`: its output may change between Rust releases, and the hash is persisted.
/// Collisions are harmless: lookups compare the full definition too.
pub(crate) fn crs_hash(def: &CrsDefinition) -> i64 {
    let mut h = Fnv1a::new();
    h.field(def.kind.code().as_bytes());
    h.field(def.text.as_bytes());
    match &def.authority {
        Some(a) => {
            h.bytes(&[1]);
            h.field(a.authority.as_bytes());
            h.field(a.code.as_bytes());
        }
        None => h.bytes(&[0]),
    }
    i64::from_ne_bytes(h.0.to_ne_bytes())
}

struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Fnv1a(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    /// Length-prefixed, so ("ab", "c") and ("a", "bc") hash differently.
    fn field(&mut self, bytes: &[u8]) {
        self.bytes(&(bytes.len() as u64).to_le_bytes());
        self.bytes(bytes);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use whereabouts_core::{AuthorityCode, CrsDefinitionKind};

    #[test]
    fn frn_bit_cast_round_trips() {
        for v in [0, 1, u64::MAX, 1 << 63, 0x0001_0000_0000_0123] {
            assert_eq!(frn_from_sql(frn_to_sql(Frn(v))), Frn(v));
        }
    }

    #[test]
    fn fnv_known_vector() {
        let mut h = Fnv1a::new();
        h.bytes(b"a");
        assert_eq!(h.0, 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn crs_hash_distinguishes_fields() {
        let a = CrsDefinition::new(CrsDefinitionKind::Wkt1, "GEOGCS[]");
        let b = CrsDefinition::new(CrsDefinitionKind::Wkt1Esri, "GEOGCS[]");
        let mut c = a.clone();
        c.authority = Some(AuthorityCode {
            authority: "EPSG".into(),
            code: "4326".into(),
        });
        let mut d = a.clone();
        d.authority = Some(AuthorityCode {
            authority: "EPSG4".into(),
            code: "326".into(),
        });
        let hashes = [a, b, c, d].map(|x| crs_hash(&x));
        for i in 0..hashes.len() {
            for j in i + 1..hashes.len() {
                assert_ne!(hashes[i], hashes[j], "{i} vs {j}");
            }
        }
        assert_eq!(
            crs_hash(&CrsDefinition::epsg(4326)),
            crs_hash(&CrsDefinition::epsg(4326))
        );
    }

    #[test]
    fn range_checks() {
        assert!(u64_to_sql(u64::MAX, "x").is_err());
        assert_eq!(u64_to_sql(5, "x").unwrap(), 5);
        assert!(u64_from_sql(-1, "x").is_err());
        assert!(volume_from_sql(i64::from(u32::MAX) + 1).is_err());
        assert_eq!(finite(Some(f64::NAN)), None);
        assert_eq!(finite(Some(1.5)), Some(1.5));
    }
}
