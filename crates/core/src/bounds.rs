//! Bounding boxes: native-CRS bounds and WGS84 extents (split at the antimeridian).

use serde::{Deserialize, Serialize};

use crate::CoreError;

/// An axis-aligned bounding box in some CRS's native units, `x` = first axis as stored by the
/// format (easting/longitude for the formats in scope; readers must not swap axes).
///
/// Invariants (checked by [`Bounds::new`] and deserialisation): all four values are finite and
/// `min <= max` on both axes. A degenerate box (a single point or line) is valid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BoundsRepr", into = "BoundsRepr")]
pub struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Bounds {
    /// Creates bounds, rejecting NaN/infinite values and inverted ranges.
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Result<Self, CoreError> {
        let all = [min_x, min_y, max_x, max_y];
        if !all.iter().all(|v| v.is_finite()) {
            return Err(CoreError::InvalidBounds(format!(
                "non-finite value in {all:?}"
            )));
        }
        if min_x > max_x || min_y > max_y {
            return Err(CoreError::InvalidBounds(format!(
                "inverted range in {all:?}"
            )));
        }
        Ok(Bounds {
            min_x,
            min_y,
            max_x,
            max_y,
        })
    }

    /// Minimum x.
    pub fn min_x(&self) -> f64 {
        self.min_x
    }

    /// Minimum y.
    pub fn min_y(&self) -> f64 {
        self.min_y
    }

    /// Maximum x.
    pub fn max_x(&self) -> f64 {
        self.max_x
    }

    /// Maximum y.
    pub fn max_y(&self) -> f64 {
        self.max_y
    }

    /// `max_x - min_x`.
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    /// `max_y - min_y`.
    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    /// The smallest box containing both.
    pub fn union(&self, other: &Bounds) -> Bounds {
        Bounds {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// `true` if the boxes share at least one point (touching edges count).
    pub fn intersects(&self, other: &Bounds) -> bool {
        self.min_x <= other.max_x
            && other.min_x <= self.max_x
            && self.min_y <= other.max_y
            && other.min_y <= self.max_y
    }
}

#[derive(Serialize, Deserialize)]
struct BoundsRepr {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl TryFrom<BoundsRepr> for Bounds {
    type Error = CoreError;

    fn try_from(r: BoundsRepr) -> Result<Self, CoreError> {
        Bounds::new(r.min_x, r.min_y, r.max_x, r.max_y)
    }
}

impl From<Bounds> for BoundsRepr {
    fn from(b: Bounds) -> Self {
        BoundsRepr {
            min_x: b.min_x,
            min_y: b.min_y,
            max_x: b.max_x,
            max_y: b.max_y,
        }
    }
}

/// A dataset/layer extent in WGS84 longitude/latitude degrees, as one or more boxes.
///
/// Extents crossing the antimeridian are split into two parts (east of and west of ±180°) so every
/// part is an ordinary `min <= max` box; the catalog stores one R*Tree row per part.
///
/// Invariants (checked by [`Wgs84Extent::new`] and deserialisation): at least one part; every part
/// lies within longitude [-180, 180] and latitude [-90, 90]. `x` = longitude, `y` = latitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Wgs84ExtentRepr", into = "Wgs84ExtentRepr")]
pub struct Wgs84Extent {
    parts: Vec<Bounds>,
}

impl Wgs84Extent {
    /// Creates an extent from its parts.
    pub fn new(parts: Vec<Bounds>) -> Result<Self, CoreError> {
        if parts.is_empty() {
            return Err(CoreError::InvalidExtent("no parts".into()));
        }
        for p in &parts {
            if p.min_x < -180.0 || p.max_x > 180.0 || p.min_y < -90.0 || p.max_y > 90.0 {
                return Err(CoreError::InvalidExtent(format!(
                    "part out of range: {p:?}"
                )));
            }
        }
        Ok(Wgs84Extent { parts })
    }

    /// Creates an extent from a GeoJSON-style `[west, south, east, north]` box: when
    /// `west > east` the box crosses the antimeridian and is split into
    /// `[west, south, 180, north]` and `[-180, south, east, north]`.
    pub fn from_west_south_east_north(
        west: f64,
        south: f64,
        east: f64,
        north: f64,
    ) -> Result<Self, CoreError> {
        if west > east {
            Wgs84Extent::new(vec![
                Bounds::new(west, south, 180.0, north)?,
                Bounds::new(-180.0, south, east, north)?,
            ])
        } else {
            Wgs84Extent::new(vec![Bounds::new(west, south, east, north)?])
        }
    }

    /// The parts (one, or two for an antimeridian split).
    pub fn parts(&self) -> &[Bounds] {
        &self.parts
    }

    /// `true` if the extent has more than one part.
    pub fn is_split(&self) -> bool {
        self.parts.len() > 1
    }

    /// `true` if any part intersects the given lon/lat box.
    pub fn intersects(&self, query: &Bounds) -> bool {
        self.parts.iter().any(|p| p.intersects(query))
    }
}

#[derive(Serialize, Deserialize)]
struct Wgs84ExtentRepr {
    parts: Vec<Bounds>,
}

impl TryFrom<Wgs84ExtentRepr> for Wgs84Extent {
    type Error = CoreError;

    fn try_from(r: Wgs84ExtentRepr) -> Result<Self, CoreError> {
        Wgs84Extent::new(r.parts)
    }
}

impl From<Wgs84Extent> for Wgs84ExtentRepr {
    fn from(e: Wgs84Extent) -> Self {
        Wgs84ExtentRepr { parts: e.parts }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn bounds_reject_nan_infinite_and_inverted() {
        assert!(Bounds::new(f64::NAN, 0.0, 1.0, 1.0).is_err());
        assert!(Bounds::new(0.0, 0.0, f64::INFINITY, 1.0).is_err());
        assert!(Bounds::new(0.0, f64::NEG_INFINITY, 1.0, 1.0).is_err());
        assert!(Bounds::new(2.0, 0.0, 1.0, 1.0).is_err());
        assert!(Bounds::new(0.0, 2.0, 1.0, 1.0).is_err());
        let point = Bounds::new(5.0, 5.0, 5.0, 5.0).unwrap();
        assert_eq!(point.width(), 0.0);
    }

    #[test]
    fn bounds_ops() {
        let a = Bounds::new(0.0, 0.0, 10.0, 10.0).unwrap();
        let b = Bounds::new(10.0, 5.0, 20.0, 6.0).unwrap();
        let c = Bounds::new(11.0, 0.0, 12.0, 1.0).unwrap();
        assert!(a.intersects(&b), "touching counts");
        assert!(!a.intersects(&c));
        assert_eq!(a.union(&c), Bounds::new(0.0, 0.0, 12.0, 10.0).unwrap());
        assert_eq!(a.height(), 10.0);
    }

    #[test]
    fn bounds_serde_validates() {
        let b = Bounds::new(1_700_000.0, 5_400_000.0, 1_800_000.0, 5_500_000.0).unwrap();
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<Bounds>(&json).unwrap(), b);
        let inverted = r#"{"min_x":1.0,"min_y":0.0,"max_x":0.0,"max_y":1.0}"#;
        assert!(serde_json::from_str::<Bounds>(inverted).is_err());
    }

    #[test]
    fn wgs84_extent_validation() {
        assert!(Wgs84Extent::new(vec![]).is_err());
        let out = Bounds::new(-181.0, 0.0, 0.0, 1.0).unwrap();
        assert!(Wgs84Extent::new(vec![out]).is_err());
        let polar = Bounds::new(-180.0, -90.0, 180.0, -60.0).unwrap();
        assert!(Wgs84Extent::new(vec![polar]).is_ok());
        let json = r#"{"parts":[{"min_x":0.0,"min_y":95.0,"max_x":1.0,"max_y":96.0}]}"#;
        assert!(serde_json::from_str::<Wgs84Extent>(json).is_err());
    }

    #[test]
    fn antimeridian_split() {
        // Fiji-like extent: 177°E .. 178°W.
        let e = Wgs84Extent::from_west_south_east_north(177.0, -21.0, -178.0, -12.0).unwrap();
        assert!(e.is_split());
        assert_eq!(
            e.parts()[0],
            Bounds::new(177.0, -21.0, 180.0, -12.0).unwrap()
        );
        assert_eq!(
            e.parts()[1],
            Bounds::new(-180.0, -21.0, -178.0, -12.0).unwrap()
        );
        assert!(e.intersects(&Bounds::new(-179.0, -18.0, -179.0, -18.0).unwrap()));
        assert!(!e.intersects(&Bounds::new(0.0, -18.0, 1.0, -17.0).unwrap()));
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Wgs84Extent>(&json).unwrap(), e);
        let simple = Wgs84Extent::from_west_south_east_north(166.0, -47.0, 179.0, -34.0).unwrap();
        assert!(!simple.is_split());
    }
}
