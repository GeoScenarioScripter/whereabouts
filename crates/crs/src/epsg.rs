//! The embedded EPSG table (`data/epsg.txt`): geodetic datums, geographic 2D/3D CRSs and
//! projected CRSs, generated from PROJ's `proj.db` by `scripts/gen_epsg.py` (record layout
//! documented there). It is parsed once, on first use, into indexes by code and by name.
//!
//! The table is our own build artefact, not user input, but it is still parsed without panics: a
//! malformed line is skipped (and a unit test checks that none are).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::model::{
    Authority, Axis, AxisDirection, AxisOrder, BaseCrs, Crs, CrsKind, Datum, Dialect, Ellipsoid,
    Parameter, PrimeMeridian, Projection, Unit, UnitKind,
};
use crate::names::{ParamClass, ParamName, ProjectionMethod, normalize_name};

/// The generated table text.
const TABLE: &str = include_str!("../data/epsg.txt");

/// A geodetic datum row.
#[derive(Debug)]
pub(crate) struct DatumRow {
    pub code: u32,
    pub name: &'static str,
    /// Semi-major axis, metres.
    pub a: f64,
    /// Inverse flattening; 0 for a sphere.
    pub rf: f64,
    /// Prime meridian, degrees east of Greenwich.
    pub pm: f64,
    pub aliases: &'static str,
}

/// A geographic (2D or 3D) CRS row.
#[derive(Debug)]
pub(crate) struct GeogRow {
    pub code: u32,
    pub name: &'static str,
    pub datum: u32,
    /// Factor converting the CRS's angular unit to degrees (1 for degrees, 0.9 for grads).
    pub to_deg: f64,
    pub axes: [u8; 2],
    pub deprecated: bool,
    pub is_3d: bool,
    pub to_wgs84: Option<[f64; 7]>,
    pub aliases: &'static str,
}

/// A projected CRS row.
#[derive(Debug)]
pub(crate) struct ProjRow {
    pub code: u32,
    pub name: &'static str,
    /// Code of the base geographic CRS.
    pub base: u32,
    /// EPSG conversion method code.
    pub method: u32,
    /// `(EPSG parameter code, value)`, angles in degrees, lengths in metres.
    pub params: Vec<(u32, f64)>,
    /// Metres per CRS linear unit.
    pub unit: f64,
    pub axes: [u8; 2],
    pub deprecated: bool,
    pub aliases: &'static str,
}

/// A CRS in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Index into [`Table::geog`].
    Geog(usize),
    /// Index into [`Table::proj`].
    Proj(usize),
}

/// The parsed table with its indexes.
#[derive(Debug)]
pub(crate) struct Table {
    /// Provenance line (EPSG dataset version).
    pub source: &'static str,
    pub datums: Vec<DatumRow>,
    pub geog: Vec<GeogRow>,
    pub proj: Vec<ProjRow>,
    datum_by_code: HashMap<u32, usize>,
    by_code: HashMap<u32, Entry>,
    /// [`key`] of every CRS name and alias → entries.
    by_name: HashMap<String, Vec<Entry>>,
    /// [`key`] of every datum name and alias → datum codes.
    datum_by_name: HashMap<String, Vec<u32>>,
    /// Lines that did not parse (expected: 0).
    pub skipped: usize,
}

/// The table, parsed on first use.
pub(crate) fn table() -> &'static Table {
    static TABLE_CELL: OnceLock<Table> = OnceLock::new();
    TABLE_CELL.get_or_init(|| Table::parse(TABLE))
}

impl Table {
    fn parse(text: &'static str) -> Self {
        let mut t = Table {
            source: "",
            datums: Vec::new(),
            geog: Vec::new(),
            proj: Vec::new(),
            datum_by_code: HashMap::new(),
            by_code: HashMap::new(),
            by_name: HashMap::new(),
            datum_by_name: HashMap::new(),
            skipped: 0,
        };
        for line in text.lines() {
            if let Some(comment) = line.strip_prefix('#') {
                if t.source.is_empty() && comment.trim_start().starts_with("Source:") {
                    t.source = comment.trim();
                }
                continue;
            }
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&'static str> = line.split('|').collect();
            let ok = match fields.first().copied() {
                Some("D") => parse_datum(&fields).map(|d| t.datums.push(d)).is_some(),
                Some("G") => parse_geog(&fields).map(|g| t.geog.push(g)).is_some(),
                Some("P") => parse_proj(&fields).map(|p| t.proj.push(p)).is_some(),
                _ => false,
            };
            if !ok {
                t.skipped += 1;
            }
        }
        for (i, d) in t.datums.iter().enumerate() {
            t.datum_by_code.insert(d.code, i);
            for name in std::iter::once(d.name).chain(split_aliases(d.aliases)) {
                push_unique(&mut t.datum_by_name, key(name), d.code);
            }
        }
        for (i, g) in t.geog.iter().enumerate() {
            t.by_code.insert(g.code, Entry::Geog(i));
            for name in std::iter::once(g.name).chain(split_aliases(g.aliases)) {
                push_unique(&mut t.by_name, key(name), Entry::Geog(i));
            }
        }
        for (i, p) in t.proj.iter().enumerate() {
            t.by_code.insert(p.code, Entry::Proj(i));
            for name in std::iter::once(p.name).chain(split_aliases(p.aliases)) {
                push_unique(&mut t.by_name, key(name), Entry::Proj(i));
            }
        }
        t
    }

    /// The entry for an EPSG code.
    pub fn get(&self, code: u32) -> Option<Entry> {
        self.by_code.get(&code).copied()
    }

    /// Entries whose name or an alias has this [`key`].
    pub fn by_name(&self, key: &str) -> &[Entry] {
        self.by_name.get(key).map_or(&[], Vec::as_slice)
    }

    /// Datum codes whose name or an alias has this [`key`].
    pub fn datums_named(&self, key: &str) -> &[u32] {
        self.datum_by_name.get(key).map_or(&[], Vec::as_slice)
    }

    /// A datum row by code.
    pub fn datum(&self, code: u32) -> Option<&DatumRow> {
        self.datum_by_code
            .get(&code)
            .and_then(|&i| self.datums.get(i))
    }

    /// The EPSG code of an entry.
    pub fn code(&self, entry: Entry) -> Option<u32> {
        match entry {
            Entry::Geog(i) => self.geog.get(i).map(|g| g.code),
            Entry::Proj(i) => self.proj.get(i).map(|p| p.code),
        }
    }

    /// The name of an entry.
    pub fn name(&self, entry: Entry) -> Option<&'static str> {
        match entry {
            Entry::Geog(i) => self.geog.get(i).map(|g| g.name),
            Entry::Proj(i) => self.proj.get(i).map(|p| p.name),
        }
    }

    /// Whether an entry is deprecated in EPSG.
    pub fn deprecated(&self, entry: Entry) -> bool {
        match entry {
            Entry::Geog(i) => self.geog.get(i).is_some_and(|g| g.deprecated),
            Entry::Proj(i) => self.proj.get(i).is_some_and(|p| p.deprecated),
        }
    }

    /// The geodetic datum code of an entry (of the base CRS for projected CRSs).
    pub fn datum_of(&self, entry: Entry) -> Option<u32> {
        match entry {
            Entry::Geog(i) => self.geog.get(i).map(|g| g.datum),
            Entry::Proj(i) => {
                let base = self.proj.get(i)?.base;
                match self.get(base)? {
                    Entry::Geog(g) => self.geog.get(g).map(|g| g.datum),
                    Entry::Proj(_) => None,
                }
            }
        }
    }

    /// Builds the [`Crs`] model of an entry.
    pub fn crs(&self, entry: Entry) -> Option<Crs> {
        match entry {
            Entry::Geog(i) => self.geog_crs(self.geog.get(i)?),
            Entry::Proj(i) => self.proj_crs(self.proj.get(i)?),
        }
    }

    fn geog_crs(&self, g: &GeogRow) -> Option<Crs> {
        let datum = self.datum(g.datum)?;
        let mut crs = Crs::empty(CrsKind::Geographic, Dialect::EpsgTable, g.name.to_owned());
        crs.authority = Some(epsg_authority(g.code));
        crs.datum = Some(model_datum(datum, g.to_wgs84)?);
        crs.angular_unit = Some(angular_unit(g.to_deg));
        crs.axes = axes_from_letters(g.axes, crs.angular_unit.as_ref());
        crs.axis_order = AxisOrder::from_axes(&crs.axes);
        Some(crs)
    }

    fn proj_crs(&self, p: &ProjRow) -> Option<Crs> {
        let base = match self.get(p.base)? {
            Entry::Geog(i) => self.geog.get(i)?,
            Entry::Proj(_) => return None,
        };
        let datum = self.datum(base.datum)?;
        let mut crs = Crs::empty(CrsKind::Projected, Dialect::EpsgTable, p.name.to_owned());
        crs.authority = Some(epsg_authority(p.code));
        crs.base = Some(BaseCrs {
            name: base.name.to_owned(),
            authority: Some(epsg_authority(base.code)),
        });
        crs.datum = Some(model_datum(datum, base.to_wgs84)?);
        crs.angular_unit = Some(angular_unit(base.to_deg));
        let linear = Unit {
            name: if p.unit == 1.0 {
                "metre".to_owned()
            } else {
                format!("{} metre", p.unit)
            },
            kind: UnitKind::Linear,
            factor: p.unit,
        };
        let method = ProjectionMethod::from_epsg(p.method).unwrap_or(ProjectionMethod::Other);
        let parameters = p
            .params
            .iter()
            .map(|&(code, value)| {
                let name = ParamName::from_epsg(code).unwrap_or(ParamName::Other);
                let unit = match name.class() {
                    ParamClass::Angular => Some(Unit::degree()),
                    ParamClass::Linear => Some(Unit::metre()),
                    ParamClass::Scale => Some(Unit::unity()),
                    ParamClass::Unknown => None,
                };
                Parameter {
                    name,
                    original_name: format!("EPSG parameter {code}"),
                    value,
                    unit,
                    authority: Some(epsg_authority(code)),
                }
            })
            .collect();
        crs.projection = Some(Projection {
            method,
            method_name: format!("EPSG method {}", p.method),
            method_authority: Some(epsg_authority(p.method)),
            parameters,
        });
        crs.axes = axes_from_letters(p.axes, Some(&linear));
        crs.axis_order = AxisOrder::from_axes(&crs.axes);
        crs.linear_unit = Some(linear);
        Some(crs)
    }
}

/// Lookup key for CRS and datum names: [`normalize_name`] with `+` kept (as `plus`), so
/// `CH1903` and `CH1903+` stay different datums.
pub(crate) fn key(name: &str) -> String {
    normalize_name(&name.replace('+', "plus"))
}

fn push_unique<T: PartialEq>(map: &mut HashMap<String, Vec<T>>, key: String, value: T) {
    if key.is_empty() {
        return;
    }
    let list = map.entry(key).or_default();
    if !list.contains(&value) {
        list.push(value);
    }
}

fn split_aliases(aliases: &'static str) -> impl Iterator<Item = &'static str> {
    aliases.split(';').filter(|a| !a.is_empty())
}

fn epsg_authority(code: u32) -> Authority {
    Authority {
        name: "EPSG".to_owned(),
        code: code.to_string(),
    }
}

fn angular_unit(to_deg: f64) -> Unit {
    if (to_deg - 0.9).abs() < 1e-12 {
        Unit {
            name: "grad".to_owned(),
            kind: UnitKind::Angular,
            factor: std::f64::consts::PI / 200.0,
        }
    } else if (to_deg - 1.0).abs() < 1e-12 {
        Unit::degree()
    } else {
        Unit {
            name: format!("{to_deg} degree"),
            kind: UnitKind::Angular,
            factor: to_deg.to_radians(),
        }
    }
}

fn model_datum(d: &DatumRow, to_wgs84: Option<[f64; 7]>) -> Option<Datum> {
    Some(Datum {
        name: d.name.to_owned(),
        ellipsoid: Some(Ellipsoid::checked(String::new(), d.a, d.rf, None).ok()?),
        prime_meridian: PrimeMeridian {
            name: if d.pm == 0.0 {
                "Greenwich".to_owned()
            } else {
                String::new()
            },
            longitude: d.pm,
            authority: None,
        },
        to_wgs84,
        authority: Some(epsg_authority(d.code)),
    })
}

fn axes_from_letters(letters: [u8; 2], unit: Option<&Unit>) -> Vec<Axis> {
    letters
        .iter()
        .map(|&l| Axis {
            name: String::new(),
            direction: match l {
                b'e' => AxisDirection::East,
                b'w' => AxisDirection::West,
                b'n' => AxisDirection::North,
                b's' => AxisDirection::South,
                _ => AxisDirection::Other("other".to_owned()),
            },
            unit: unit.cloned(),
        })
        .collect()
}

fn num(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn code(s: &str) -> Option<u32> {
    s.parse().ok()
}

fn axes(s: &str) -> Option<[u8; 2]> {
    match s.as_bytes() {
        [a, b] => Some([*a, *b]),
        [] => Some(*b"en"),
        _ => None,
    }
}

fn parse_datum(f: &[&'static str]) -> Option<DatumRow> {
    let [_, c, name, a, rf, pm, aliases] = f else {
        return None;
    };
    Some(DatumRow {
        code: code(c)?,
        name,
        a: num(a)?,
        rf: num(rf)?,
        pm: num(pm)?,
        aliases,
    })
}

fn parse_geog(f: &[&'static str]) -> Option<GeogRow> {
    let [_, c, name, datum, to_deg, ax, flags, towgs84, aliases] = f else {
        return None;
    };
    let to_wgs84 = if towgs84.is_empty() {
        None
    } else {
        let v: Vec<f64> = towgs84.split(',').map(num).collect::<Option<_>>()?;
        Some(<[f64; 7]>::try_from(v).ok()?)
    };
    Some(GeogRow {
        code: code(c)?,
        name,
        datum: code(datum)?,
        to_deg: num(to_deg)?,
        axes: axes(ax)?,
        deprecated: flags.contains('d'),
        is_3d: flags.contains('3'),
        to_wgs84,
        aliases,
    })
}

fn parse_proj(f: &[&'static str]) -> Option<ProjRow> {
    let [_, c, name, base, method, params, unit, ax, flags, aliases] = f else {
        return None;
    };
    let params = if params.is_empty() {
        Vec::new()
    } else {
        params
            .split(',')
            .map(|kv| {
                let (k, v) = kv.split_once(':')?;
                Some((code(k)?, num(v)?))
            })
            .collect::<Option<Vec<_>>>()?
    };
    let unit = num(unit).filter(|u| *u > 0.0)?;
    Some(ProjRow {
        code: code(c)?,
        name,
        base: code(base)?,
        method: code(method)?,
        params,
        unit,
        axes: axes(ax)?,
        deprecated: flags.contains('d'),
        aliases,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn every_line_parses() {
        let t = table();
        assert_eq!(t.skipped, 0);
        assert!(t.source.contains("EPSG Geodetic Parameter Dataset"));
        assert!(t.datums.len() > 500, "{}", t.datums.len());
        assert!(t.geog.len() > 800, "{}", t.geog.len());
        assert!(t.proj.len() > 5000, "{}", t.proj.len());
        // Every CRS's datum (and every projected CRS's base) resolves.
        for g in &t.geog {
            assert!(t.datum(g.datum).is_some(), "datum of {}", g.code);
        }
        for p in &t.proj {
            assert!(
                matches!(t.get(p.base), Some(Entry::Geog(_))),
                "base of {}",
                p.code
            );
        }
    }

    #[test]
    fn lookups() {
        let t = table();
        let e = t.get(32633).unwrap();
        assert_eq!(t.name(e), Some("WGS 84 / UTM zone 33N"));
        assert_eq!(t.datum_of(e), Some(6326));
        assert!(t.by_name(&key("WGS_1984_UTM_Zone_33N")).contains(&e));
        assert!(t.datums_named(&key("D_WGS_1984")).contains(&6326));
        let crs = t.crs(e).unwrap();
        assert_eq!(crs.kind, CrsKind::Projected);
        assert_eq!(crs.epsg(), Some(32633));
        let p = crs.projection.unwrap();
        assert_eq!(p.method, ProjectionMethod::TransverseMercator);
        assert_eq!(
            p.param(ParamName::LongitudeOfOrigin).unwrap().degrees(),
            Some(15.0)
        );
        // Grads and a non-Greenwich prime meridian.
        let ntf = t.crs(t.get(4807).unwrap()).unwrap();
        assert!(!ntf.angular_unit.as_ref().unwrap().is_degree());
        assert!((ntf.datum.unwrap().prime_meridian.longitude - 2.33722917).abs() < 1e-9);
    }
}
