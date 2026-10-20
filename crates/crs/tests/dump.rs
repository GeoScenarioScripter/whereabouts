#![allow(clippy::unwrap_used)]
use std::path::Path;

#[test]
fn dump() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        let mut files: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for f in files {
            let bytes = std::fs::read(&f).unwrap();
            let name = format!(
                "{}/{}",
                d.file_name().unwrap().to_string_lossy(),
                f.file_name().unwrap().to_string_lossy()
            );
            match whereabouts_crs::parse_bytes(&bytes) {
                Ok(c) => {
                    let h = c.horizontal();
                    println!(
                        "{name:40} {:?} {:?} {:?} lin={:?} ang={:?} epsg={:?} order={:?} comps={}",
                        c.kind,
                        c.dialect,
                        h.and_then(|h| h.projection.as_ref()).map(|p| p.method),
                        h.or(c.components.last())
                            .unwrap_or(&c)
                            .linear_unit
                            .as_ref()
                            .map(|u| (u.name.clone(), u.factor)),
                        h.and_then(|h| h.angular_unit.as_ref())
                            .map(|u| (u.name.clone(), u.factor)),
                        c.epsg(),
                        c.axis_order,
                        c.components.len()
                    );
                }
                Err(e) => println!("{name:40} ERROR {e}"),
            }
        }
    }
}
