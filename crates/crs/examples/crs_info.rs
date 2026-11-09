//! Prints what whereabouts-crs makes of a CRS: parse result, EPSG identification and a sample
//! bbox transform. A debugging aid for CRS issues (and for G2 disagreements).
//!
//! ```text
//! cargo run -p whereabouts-crs --example crs_info -- EPSG:2193
//! cargo run -p whereabouts-crs --example crs_info -- path/to/file.prj [minx miny maxx maxy]
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use whereabouts_core::Bounds;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: crs_info EPSG:<code> | <file> [minx miny maxx maxy]");
        std::process::exit(2);
    };
    let crs = match input.strip_prefix("EPSG:") {
        Some(code) => whereabouts_crs::epsg_crs(code.parse().expect("numeric code")),
        None => {
            let bytes = std::fs::read(input).expect("readable file");
            match whereabouts_crs::parse_bytes(&bytes) {
                Ok(c) => Some(c),
                Err(e) => {
                    println!("parse error: {e}");
                    None
                }
            }
        }
    };
    let Some(crs) = crs else {
        println!("no CRS");
        return;
    };
    println!("{crs:#?}");
    println!("identify: {:?}", whereabouts_crs::identify(&crs));
    let nums: Vec<f64> = args[1..].iter().filter_map(|a| a.parse().ok()).collect();
    if let [a, b, c, d] = nums[..] {
        let bounds = Bounds::new(a, b, c, d).expect("valid bounds");
        println!(
            "wgs84: {:?}",
            whereabouts_crs::transform_bounds(&bounds, &crs)
        );
    }
}
