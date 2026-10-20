//! Shared helpers for the fixture-driven tests: listing `tests/data`.

#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

/// One fixture file.
#[derive(Debug)]
pub struct Fixture {
    /// Path relative to `tests/data`, with `/` separators (`epsg-32633/esri.prj`).
    pub id: String,
    /// The containing directory name (`epsg-32633`, `handwritten`).
    pub dir: String,
    /// The file name (`esri.prj`).
    pub file: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
}

pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
}

/// Every fixture file under `tests/data/<dir>/`, sorted (top-level files such as `README.md`
/// and `fetch.sh` are not fixtures).
pub fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        let dir_name = dir.file_name().unwrap().to_string_lossy().into_owned();
        for f in files {
            let file = f.file_name().unwrap().to_string_lossy().into_owned();
            out.push(Fixture {
                id: format!("{dir_name}/{file}"),
                dir: dir_name.clone(),
                file,
                bytes: std::fs::read(&f).unwrap(),
            });
        }
    }
    out
}
