//! Deterministic synthetic volumes for benchmarks and tests.
//!
//! Shaped after the CI runner's `C:` measured in stage01/1.3 (1.34M records, 15.5 % directories,
//! 24.6-byte average WTF-8 name): a directory tree up to ~14 levels deep with sibling locality,
//! names mixing words, numbers, camera/tile counters, hashes, GUIDs and WinSxS-style long names,
//! a Windows-like extension mix with a sprinkling of GIS formats, and ~2 % non-ASCII names
//! (synthetic average: ~24 bytes per name).
//! Record numbers are dense and ascending (like MFT enumeration order); sequence numbers vary.
//!
//! Behind the `synth` feature (dev-dependencies only). Not part of the production API: it exists
//! so benchmarks (here and in stage01/4.3) and
//! property tests share one realistic, reproducible name distribution.

use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};

/// FRN of the synthetic volume's root (NTFS record 5, sequence 5).
pub const ROOT: Frn = Frn(0x0005_0000_0000_0005);

const WORDS: &[&str] = &[
    "program",
    "files",
    "windows",
    "system",
    "microsoft",
    "data",
    "cache",
    "temp",
    "user",
    "app",
    "config",
    "assets",
    "images",
    "icons",
    "src",
    "lib",
    "include",
    "node",
    "modules",
    "python",
    "site",
    "packages",
    "roads",
    "rivers",
    "parcels",
    "buildings",
    "landuse",
    "elevation",
    "dem",
    "ortho",
    "imagery",
    "survey",
    "project",
    "backup",
    "archive",
    "report",
    "final",
    "draft",
    "map",
    "layer",
    "export",
    "import",
    "test",
    "build",
    "debug",
    "release",
    "x64",
    "common",
    "shared",
    "resources",
    "fonts",
    "drivers",
    "logs",
    "update",
    "package",
    "manifest",
    "client",
    "server",
    "core",
    "utils",
    "helper",
    "main",
    "index",
    "style",
    "theme",
    "photo",
    "scan",
    "document",
    "invoice",
    "contour",
    "boundary",
    "census",
    "zoning",
    "flood",
    "hydro",
    "soil",
    "geology",
    "transport",
    "utility",
    "water",
    "sewer",
    "power",
    "tile",
    "mosaic",
    "lidar",
    "points",
    "grid",
    "raster",
    "vector",
    "model",
    "terrain",
    "slope",
    "hillshade",
    "canopy",
    "trees",
    "parks",
    "schools",
    "address",
    "street",
    "county",
    "region",
    "north",
    "south",
    "east",
    "west",
    "zone",
    "settings",
    "local",
    "roaming",
    "profile",
    "desktop",
    "downloads",
    "music",
    "video",
    "game",
    "steam",
    "office",
    "adobe",
    "google",
    "chrome",
    "mozilla",
    "edge",
    "runtime",
    "dotnet",
    "java",
    "sdk",
    "tools",
    "bin",
    "obj",
    "dist",
    "vendor",
    "plugins",
];

const NON_ASCII: &[&str] = &[
    "Straße",
    "données",
    "Ōtautahi",
    "São_Paulo",
    "Zürich",
    "Москва",
    "地图",
    "データ",
    "naïve",
    "café",
    "Ελλάδα",
    "Kraków",
    "Señal",
    "Ærø",
    "İzmir",
];

/// (extension, weight); `""` = no extension.
const EXTS: &[(&str, u32)] = &[
    ("dll", 80),
    ("png", 70),
    ("js", 60),
    ("xml", 40),
    ("", 40),
    ("json", 30),
    ("txt", 30),
    ("jpg", 30),
    ("exe", 25),
    ("h", 25),
    ("mui", 25),
    ("manifest", 25),
    ("py", 25),
    ("pyc", 20),
    ("cpp", 15),
    ("c", 15),
    ("cat", 15),
    ("svg", 15),
    ("html", 15),
    ("dat", 15),
    ("ts", 15),
    ("css", 10),
    ("ini", 10),
    ("log", 10),
    ("md", 10),
    ("cs", 10),
    ("gif", 8),
    ("ico", 8),
    ("ttf", 8),
    ("sys", 8),
    ("csv", 5),
    ("zip", 5),
    ("pdf", 5),
    ("shp", 4),
    ("shx", 4),
    ("dbf", 4),
    ("prj", 4),
    ("tif", 4),
    ("docx", 3),
    ("xlsx", 3),
    ("cpg", 2),
    ("tfw", 2),
    ("gpkg", 2),
    ("geojson", 1),
    ("kml", 1),
    ("las", 1),
];

/// SplitMix64: tiny, fast, good enough for test data.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 1000 < percent * 10
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items.get(self.below(items.len())).copied().unwrap_or("x")
    }
}

/// Iterator over the records of a synthetic volume (see the module docs). The root itself is not
/// yielded, as with NTFS enumeration; use [`ROOT`].
#[derive(Debug, Clone)]
pub struct SynthVolume {
    rng: Rng,
    remaining: usize,
    next_record: u64,
    /// (FRN, depth) of every directory so far, root first.
    dirs: Vec<(Frn, u8)>,
    ext_total: u32,
}

/// The records of a synthetic volume with `count` entries, reproducible from `seed`.
#[must_use]
pub fn volume(count: usize, seed: u64) -> SynthVolume {
    SynthVolume {
        rng: Rng(seed ^ 0x5EED_0000_0000_0001),
        remaining: count,
        next_record: 64,
        dirs: vec![(ROOT, 0)],
        ext_total: EXTS.iter().map(|(_, w)| w).sum(),
    }
}

impl SynthVolume {
    fn parent(&mut self) -> (Frn, u8) {
        let n = self.dirs.len();
        let idx = if self.rng.chance(75) {
            n - 1 - self.rng.below(n.min(32))
        } else {
            self.rng.below(n)
        };
        let pick = self.dirs.get(idx).copied().unwrap_or((ROOT, 0));
        if pick.1 >= 14 {
            // Too deep: hang it off one of the early, shallow directories instead.
            return self
                .dirs
                .get(self.rng.below(n.min(64)))
                .copied()
                .unwrap_or((ROOT, 0));
        }
        pick
    }

    fn words(&mut self, max: usize) -> String {
        let count = 1 + self.rng.below(max);
        let sep = ["_", "-", " ", "", "."][self.rng.below(5)];
        let style = self.rng.below(10);
        let mut out = String::new();
        for k in 0..count {
            if k > 0 {
                out.push_str(sep);
            }
            let w = if self.rng.chance(2) {
                self.rng.pick(NON_ASCII)
            } else {
                self.rng.pick(WORDS)
            };
            match style {
                0..=4 => out.push_str(w),
                5..=7 => {
                    let mut cs = w.chars();
                    if let Some(c) = cs.next() {
                        out.extend(c.to_uppercase());
                        out.push_str(cs.as_str());
                    }
                }
                8 => out.push_str(&w.to_uppercase()),
                _ if k == 0 => out.push_str(w),
                _ => {
                    let mut cs = w.chars();
                    if let Some(c) = cs.next() {
                        out.extend(c.to_uppercase());
                        out.push_str(cs.as_str());
                    }
                }
            }
        }
        if self.rng.chance(40) {
            if self.rng.chance(50) {
                out.push_str(&format!("_{}", 2000 + self.rng.below(27)));
            } else {
                out.push_str(&format!("{}", self.rng.below(10_000)));
            }
        }
        out
    }

    fn hex(&mut self, len: usize) -> String {
        let mut s = String::with_capacity(len);
        while s.len() < len {
            s.push_str(&format!("{:016x}", self.rng.next()));
        }
        s.truncate(len);
        s
    }

    fn dir_name(&mut self) -> String {
        match self.rng.below(100) {
            0..=2 => format!(
                "amd64_microsoft-windows-{}-{}_31bf3856ad364e35_10.0.{}.{}_none_{}",
                self.rng.pick(WORDS),
                self.rng.pick(WORDS),
                19041 + self.rng.below(4000),
                self.rng.below(5000),
                self.hex(16)
            ),
            3..=4 => self.hex(2),
            7..=9 => format!(
                "v{}.{}.{}",
                self.rng.below(20),
                self.rng.below(10),
                self.rng.below(100)
            ),
            _ => self.words(4),
        }
    }

    fn file_stem(&mut self) -> String {
        match self.rng.below(100) {
            0..=7 => format!(
                "IMG_{}_{:06}",
                20100101 + self.rng.below(160_000),
                self.rng.below(1_000_000)
            ),
            8..=11 => format!("DSC{:05}", self.rng.below(100_000)),
            12..=17 => format!("tile_{}_{}", self.rng.below(4096), self.rng.below(4096)),
            18..=21 => format!("part-{:05}", self.rng.below(100_000)),
            22..=31 => {
                let len = [16, 32, 40, 40][self.rng.below(4)];
                self.hex(len)
            }
            32..=35 => format!(
                "{{{}-{}-{}-{}-{}}}",
                self.hex(8),
                self.hex(4),
                self.hex(4),
                self.hex(4),
                self.hex(12)
            )
            .to_uppercase(),
            _ => self.words(5),
        }
    }

    fn ext(&mut self) -> &'static str {
        let mut r = (self.rng.next() % u64::from(self.ext_total)) as u32;
        for (e, w) in EXTS {
            if r < *w {
                return e;
            }
            r -= w;
        }
        ""
    }
}

impl Iterator for SynthVolume {
    type Item = FileRecord;

    fn next(&mut self) -> Option<FileRecord> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let record = self.next_record;
        self.next_record += 1;
        let frn = Frn::from_parts(record, 1 + (record % 5) as u16).unwrap_or(Frn(record));
        let (parent, depth) = self.parent();
        let is_dir = self.dirs.len() < 16 || self.rng.chance(15);
        let (name, mut attrs) = if is_dir {
            self.dirs.push((frn, depth + 1));
            (self.dir_name(), FileAttributes::DIRECTORY)
        } else {
            let stem = self.file_stem();
            let name = match self.ext() {
                "" => stem,
                e => format!("{stem}.{e}"),
            };
            (name, FileAttributes::ARCHIVE)
        };
        if self.rng.chance(1) {
            attrs |= FileAttributes::HIDDEN | FileAttributes::SYSTEM;
        }
        Some(FileRecord {
            frn,
            parent,
            name: FileName::from(name),
            attrs: FileAttributes(attrs),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
