//! Read-only SQLite table b-trees, not a SQL engine. All I/O uses the dataset budget.
//! Layout: <https://www.sqlite.org/fileformat.html>. No locks, journals, or SQL execution.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use whereabouts_core::{Format, ReadError};

use crate::io::SourceFile;

type Result<T> = std::result::Result<T, ReadError>;

pub(super) fn bad(reason: impl Into<String>) -> ReadError {
    ReadError::Malformed {
        format: Format::GeoPackage,
        reason: reason.into(),
    }
}

fn slice(data: &[u8], at: usize, n: usize) -> Result<&[u8]> {
    data.get(
        at..at
            .checked_add(n)
            .ok_or_else(|| bad("SQLite offset overflow"))?,
    )
    .ok_or_else(|| bad("truncated SQLite value"))
}

fn be16(data: &[u8], at: usize) -> Result<usize> {
    let b = slice(data, at, 2)?;
    Ok(usize::from(u16::from_be_bytes([b[0], b[1]])))
}

fn be32(data: &[u8], at: usize) -> Result<u32> {
    let b = slice(data, at, 4)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn varint(data: &[u8], at: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for n in 0..9 {
        let b = *data
            .get(*at)
            .ok_or_else(|| bad("truncated SQLite varint"))?;
        *at += 1;
        if n == 8 {
            return Ok((v << 8) | u64::from(b));
        }
        v = (v << 7) | u64::from(b & 127);
        if b & 128 == 0 {
            return Ok(v);
        }
    }
    Err(bad("invalid SQLite varint"))
}

#[derive(Debug, Clone)]
pub(super) enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub(super) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }
    pub(super) fn int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub(super) fn number(&self) -> Option<f64> {
        match self {
            Self::Int(v) => Some(*v as f64),
            Self::Real(v) => Some(*v),
            _ => None,
        }
    }
    pub(super) fn blob(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Table {
    root: u32,
    columns: Vec<String>,
    rowid_column: Option<usize>,
}

impl Table {
    pub(super) fn get<'a>(&self, row: &'a [Value], column: &str) -> &'a Value {
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(column))
            .and_then(|i| {
                let v = row.get(i)?;
                if matches!(v, Value::Null) && self.rowid_column == Some(i) {
                    row.last()
                } else {
                    Some(v)
                }
            })
            .unwrap_or(&Value::Null)
    }
}

#[derive(Debug)]
pub(super) struct Database<'a> {
    file: SourceFile<'a>,
    size: usize,
    usable: usize,
    encoding: u32,
    header: Vec<u8>,
    start: Instant,
    time: Duration,
    pub(super) tables: BTreeMap<String, Table>,
}

impl<'a> Database<'a> {
    pub(super) fn open(mut file: SourceFile<'a>, time: Duration) -> Result<Self> {
        let start = Instant::now();
        let h = file.read_prefix(100)?;
        if slice(&h, 0, 16)? != b"SQLite format 3\0" {
            return Err(bad("not a SQLite database"));
        }
        let size = match be16(&h, 16)? {
            1 => 65536,
            s => s,
        };
        if !(512..=65536).contains(&size) || !size.is_power_of_two() {
            return Err(bad("invalid SQLite page size"));
        }
        let usable = size - usize::from(*h.get(20).ok_or_else(|| bad("short SQLite header"))?);
        if usable < 480 || file.len() < size as u64 || !file.len().is_multiple_of(size as u64) {
            return Err(bad("truncated SQLite pages or invalid reserved bytes"));
        }
        let encoding = be32(&h, 56)?;
        if !(1..=3).contains(&encoding) {
            return Err(bad("invalid SQLite text encoding"));
        }
        // GeoPackage application_id: old GP10 / GP11 files remain readable.
        if !matches!(be32(&h, 68)?, 0x4750_4b47 | 0x4750_3130 | 0x4750_3131) {
            return Err(bad("SQLite application_id is not GeoPackage"));
        }
        let mut db = Self {
            file,
            size,
            usable,
            encoding,
            header: h,
            start,
            time,
            tables: BTreeMap::new(),
        };
        let schema = db.scan(1, true, None)?;
        for row in schema {
            if row.first().and_then(Value::text) != Some("table") {
                continue;
            }
            let name = row
                .get(1)
                .and_then(Value::text)
                .ok_or_else(|| bad("invalid schema name"))?;
            let root = row
                .get(3)
                .and_then(Value::int)
                .and_then(|v| u32::try_from(v).ok());
            let sql = row.get(4).and_then(Value::text);
            if let (Some(root), Some(sql)) = (root.filter(|v| *v > 0), sql) {
                // Unrecognized user-table SQL must not hide unrelated GeoPackage metadata.
                // Such tables can still be counted; metadata access will report missing columns.
                let (columns, rowid_column) = columns(sql).unwrap_or_default();
                db.tables.insert(
                    name.to_owned(),
                    Table {
                        root,
                        columns,
                        rowid_column,
                    },
                );
            }
        }
        Ok(db)
    }

    fn check_time(&self) -> Result<()> {
        if self.start.elapsed() >= self.time {
            Err(bad("GeoPackage time budget exhausted"))
        } else {
            Ok(())
        }
    }

    pub(super) fn unchanged(&mut self) -> Result<bool> {
        Ok(self.file.read_at(0, 100)? == self.header)
    }

    fn page(&mut self, n: u32) -> Result<Vec<u8>> {
        self.check_time()?;
        if n == 0 {
            return Err(bad("zero SQLite page pointer"));
        }
        self.file
            .read_at(u64::from(n - 1) * self.size as u64, self.size)
    }

    pub(super) fn rows(&mut self, name: &str) -> Result<(Table, Vec<Vec<Value>>)> {
        let t = self
            .tables
            .get(name)
            .cloned()
            .ok_or_else(|| bad(format!("missing table {name}")))?;
        let rows = self.scan(t.root, true, None)?;
        Ok((t, rows))
    }

    pub(super) fn count(&mut self, name: &str) -> Result<u64> {
        let t = self
            .tables
            .get(name)
            .ok_or_else(|| bad(format!("missing table {name}")))?;
        // Counts leaf cells without reading or allocating any feature payload/overflow pages.
        Ok(self.scan(t.root, false, None)?.len() as u64)
    }

    pub(super) fn blob_prefixes(&mut self, name: &str, column: &str) -> Result<Vec<Vec<u8>>> {
        let t = self
            .tables
            .get(name)
            .ok_or_else(|| bad("missing feature table"))?;
        let column = t
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(column))
            .ok_or_else(|| bad("missing geometry column in feature table"))?;
        let rows = self.scan(t.root, true, Some(column))?;
        Ok(rows
            .into_iter()
            .filter_map(|mut r| match r.pop() {
                Some(Value::Blob(v)) => Some(v),
                _ => None,
            })
            .collect())
    }

    fn scan(
        &mut self,
        root: u32,
        records: bool,
        blob_column: Option<usize>,
    ) -> Result<Vec<Vec<Value>>> {
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        while let Some(n) = pending.pop() {
            if !seen.insert(n) {
                return Err(bad("cycle or repeated page in SQLite table"));
            }
            let p = self.page(n)?;
            let h = if n == 1 { 100 } else { 0 };
            let kind = *p.get(h).ok_or_else(|| bad("missing b-tree header"))?;
            let count = be16(&p, h + 3)?;
            let header = match kind {
                5 => 12,
                13 => 8,
                _ => {
                    return Err(bad(
                        "unsupported SQLite table b-tree (WITHOUT ROWID or corrupt page)",
                    ));
                }
            };
            slice(&p, h + header, count * 2)?;
            if kind == 5 {
                pending.push(be32(&p, h + 8)?);
            }
            for i in 0..count {
                self.check_time()?;
                if out.len() >= 100_000 {
                    return Err(bad("GeoPackage table row limit (100000) exhausted"));
                }
                let mut at = be16(&p, h + header + i * 2)?;
                if at < h + header + count * 2 || at >= self.usable {
                    return Err(bad("invalid SQLite cell pointer"));
                }
                if kind == 5 {
                    pending.push(be32(&p, at)?);
                    continue;
                }
                if !records {
                    out.push(Vec::new());
                    continue;
                }
                let len = usize::try_from(varint(&p, &mut at)?)
                    .map_err(|_| bad("SQLite payload length overflow"))?;
                let rowid = varint(&p, &mut at)? as i64;
                if let Some(column) = blob_column {
                    let prefix = self.payload_range(&p, at, len, 0, len.min(9))?;
                    let mut cursor = 0;
                    let header_len = usize::try_from(varint(&prefix, &mut cursor)?)
                        .map_err(|_| bad("record header length overflow"))?;
                    let header = self.payload_range(&p, at, len, 0, header_len)?;
                    let mut offset = header_len;
                    let mut value = Value::Null;
                    for i in 0..=column {
                        if cursor >= header_len {
                            return Err(bad("geometry column missing from record"));
                        }
                        let serial = varint(&header, &mut cursor)?;
                        if cursor > header_len {
                            return Err(bad("serial type exceeds header"));
                        }
                        let field_len = serial_length(serial)?;
                        if i == column && serial != 0 {
                            if serial < 12 || !serial.is_multiple_of(2) {
                                return Err(bad("geometry is not a BLOB"));
                            }
                            value = Value::Blob(self.payload_range(
                                &p,
                                at,
                                len,
                                offset,
                                field_len.min(72),
                            )?);
                        }
                        offset = offset
                            .checked_add(field_len)
                            .ok_or_else(|| bad("record field offset overflow"))?;
                        if offset > len {
                            return Err(bad("record field extends past payload"));
                        }
                    }
                    out.push(vec![value]);
                    continue;
                }
                let payload = self.payload(&p, at, len)?;
                let mut values = record(&payload, self.encoding)?;
                values.push(Value::Int(rowid));
                out.push(values);
            }
        }
        Ok(out)
    }

    fn payload(&mut self, p: &[u8], at: usize, len: usize) -> Result<Vec<u8>> {
        // Metadata rows (WKT, schema, R-tree nodes) must fit in 64 KiB. Feature blobs are never
        // read for counting. This cap prevents hostile lengths from allocating unbounded memory.
        if len > 65536 {
            return Err(bad("SQLite metadata record exceeds 64 KiB"));
        }
        self.payload_range(p, at, len, 0, len)
    }

    fn payload_range(
        &mut self,
        p: &[u8],
        at: usize,
        len: usize,
        offset: usize,
        size: usize,
    ) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| bad("payload range overflow"))?;
        if size > 65536 || end > len {
            return Err(bad("invalid or oversized metadata payload range"));
        }
        let max = self.usable - 35;
        let min = (self.usable - 12) * 32 / 255 - 23;
        let local = if len <= max {
            len
        } else {
            let k = min + (len - min) % (self.usable - 4);
            if k <= max { k } else { min }
        };
        let mut data = Vec::new();
        slice(p, at, local)?;
        if offset < local {
            data.extend_from_slice(slice(p, at + offset, end.min(local) - offset)?);
        }
        if end <= local {
            return Ok(data);
        }
        let mut next = be32(p, at + local)?;
        let mut seen = BTreeSet::new();
        let mut position = local;
        while position < end {
            if !seen.insert(next) {
                return Err(bad("cycle in SQLite overflow chain"));
            }
            self.check_time()?;
            if next == 0 {
                return Err(bad("short SQLite overflow chain"));
            }
            let n = (len - position).min(self.usable - 4);
            let page_offset = u64::from(next - 1) * self.size as u64;
            // Skip bodies through four-byte links; fetch only the requested metadata prefix.
            let link = self.file.read_at(page_offset, 4)?;
            next = be32(&link, 0)?;
            let begin = offset.max(position);
            let finish = end.min(position + n);
            if begin < finish {
                data.extend(
                    self.file
                        .read_at(page_offset + 4 + (begin - position) as u64, finish - begin)?,
                );
            }
            position += n;
        }
        if end == len && next != 0 {
            return Err(bad("overlong SQLite overflow chain"));
        }
        Ok(data)
    }
}

fn record(data: &[u8], encoding: u32) -> Result<Vec<Value>> {
    let mut pos = 0;
    let end =
        usize::try_from(varint(data, &mut pos)?).map_err(|_| bad("record header overflow"))?;
    if end < pos || end > data.len() {
        return Err(bad("invalid SQLite record header"));
    }
    let mut at = end;
    let mut values = Vec::new();
    while pos < end {
        if values.len() >= 1024 {
            return Err(bad("SQLite column limit (1024) exceeded"));
        }
        let serial = varint(data, &mut pos)?;
        if pos > end {
            return Err(bad("serial type extends past record header"));
        }
        let len = serial_length(serial)?;
        let bytes = slice(data, at, len)?;
        at += len;
        values.push(match serial {
            0 => Value::Null,
            8 => Value::Int(0),
            9 => Value::Int(1),
            1..=6 => {
                let mut v = if bytes.first().is_some_and(|b| b & 128 != 0) {
                    -1i64
                } else {
                    0
                };
                for b in bytes {
                    v = (v << 8) | i64::from(*b);
                }
                Value::Int(v)
            }
            7 => Value::Real(f64::from_be_bytes(
                bytes.try_into().map_err(|_| bad("invalid REAL"))?,
            )),
            s if s % 2 == 0 => Value::Blob(bytes.to_vec()),
            _ => Value::Text(match encoding {
                1 => std::str::from_utf8(bytes)
                    .map_err(|_| bad("invalid SQLite UTF-8"))?
                    .to_owned(),
                _ => {
                    if !bytes.len().is_multiple_of(2) {
                        return Err(bad("odd UTF-16 byte length"));
                    }
                    let units: Vec<_> = bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| {
                            if encoding == 2 {
                                u16::from_le_bytes([b[0], b[1]])
                            } else {
                                u16::from_be_bytes([b[0], b[1]])
                            }
                        })
                        .collect();
                    String::from_utf16(&units).map_err(|_| bad("invalid SQLite UTF-16"))?
                }
            }),
        });
    }
    Ok(values)
}

// Split CREATE TABLE column declarations while respecting quoted names, strings, nested type
// arguments and CHECK expressions. Constraints don't occupy record columns.
fn columns(sql: &str) -> Result<(Vec<String>, Option<usize>)> {
    let mut quoted = None;
    let start = sql
        .bytes()
        .enumerate()
        .find_map(|(i, b)| {
            if let Some(q) = quoted {
                if b == q {
                    quoted = None;
                }
            } else {
                match b {
                    b'"' | b'`' | b'\'' => quoted = Some(b),
                    b'[' => quoted = Some(b']'),
                    b'(' => return Some(i),
                    _ => {}
                }
            }
            None
        })
        .ok_or_else(|| bad("missing table column definitions"))?;
    let mut depth = 1;
    let mut quote = None;
    let mut part = start + 1;
    let mut names = Vec::new();
    let mut rowid_column = None;
    let bytes = sql.as_bytes();
    let mut i = part;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                if bytes.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            }
        } else {
            match b {
                b'\'' | b'"' | b'`' => quote = Some(b),
                b'[' => quote = Some(b']'),
                b'(' => depth += 1,
                b')' | b',' if depth == 1 => {
                    let decl = sql
                        .get(part..i)
                        .ok_or_else(|| bad("invalid column text"))?
                        .trim();
                    let name = identifier(decl)?;
                    if !["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"]
                        .contains(&name.to_ascii_uppercase().as_str())
                    {
                        let upper = decl.to_ascii_uppercase();
                        // Only the exact INTEGER type aliases rowid; INT/BIGINT do not.
                        if upper.contains("PRIMARY KEY")
                            && !upper.contains("PRIMARY KEY DESC")
                            && upper.split_whitespace().nth(1) == Some("INTEGER")
                        {
                            rowid_column = Some(names.len());
                        }
                        names.push(name);
                    }
                    if b == b')' {
                        return Ok((names, rowid_column));
                    }
                    part = i + 1;
                }
                b')' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    Err(bad("unterminated table definition"))
}

fn identifier(s: &str) -> Result<String> {
    let first = s
        .as_bytes()
        .first()
        .copied()
        .ok_or_else(|| bad("empty column declaration"))?;
    let close = match first {
        b'"' | b'\'' | b'`' => Some(first),
        b'[' => Some(b']'),
        _ => None,
    };
    if let Some(close) = close {
        let mut name = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 1;
        while let Some(&b) = bytes.get(i) {
            if b == close {
                if bytes.get(i + 1) != Some(&close) {
                    return String::from_utf8(name).map_err(|_| bad("invalid column name"));
                }
                i += 1;
            }
            name.push(b);
            i += 1;
        }
        Err(bad("unterminated quoted column name"))
    } else {
        Ok(s.split_whitespace()
            .next()
            .ok_or_else(|| bad("missing column name"))?
            .to_owned())
    }
}

fn serial_length(serial: u64) -> Result<usize> {
    let len = match serial {
        0 | 8 | 9 => 0,
        1..=4 => serial,
        5 => 6,
        6 | 7 => 8,
        10 | 11 => return Err(bad("reserved SQLite serial type")),
        s => (s - 12) / 2,
    };
    usize::try_from(len).map_err(|_| bad("SQLite field length overflow"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn varints_and_record_values() {
        for (bytes, expected) in [
            (vec![0], 0),
            (vec![127], 127),
            (vec![129, 0], 128),
            (vec![255; 9], u64::MAX),
        ] {
            let mut at = 0;
            assert_eq!(varint(&bytes, &mut at).unwrap(), expected);
            assert_eq!(at, bytes.len());
        }
        for bytes in [vec![], vec![128], vec![255; 8]] {
            assert!(varint(&bytes, &mut 0).is_err());
        }
        let row = record(&[5, 1, 17, 8, 9, 255, b'h', b'i'], 1).unwrap();
        assert_eq!(row[0].int(), Some(-1));
        assert_eq!(row[1].text(), Some("hi"));
        assert_eq!(row[2].int(), Some(0));
        assert_eq!(row[3].int(), Some(1));
        for bytes in [vec![0], vec![3, 10, 0], vec![2, 255], vec![2, 23, b'a']] {
            assert!(record(&bytes, 1).is_err());
        }
    }

    #[test]
    fn schema_names_and_nested_definitions() {
        let (cols,key) = columns("CREATE TABLE t(\"srs_id\" INTEGER NOT NULL PRIMARY KEY, [x] DOUBLE, `a``b` TEXT DEFAULT 'a,b', val DECIMAL(8,2) CHECK(val IN (1,2)), CONSTRAINT pk UNIQUE(x))").unwrap();
        assert_eq!(cols, ["srs_id", "x", "a`b", "val"]);
        assert_eq!(key, Some(0));
        assert!(columns("CREATE TABLE t (x TEXT DEFAULT 'unterminated)").is_err());
        assert!(columns("garbage").is_err());
    }
}
