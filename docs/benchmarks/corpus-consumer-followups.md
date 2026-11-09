# Required corpus-consumer test updates (outside 4.3 write scope)

Do not weaken the new oracle baselines. The following exact test-only updates are requested
for merge review; 4.3 has not applied them because `crates/readers/**` and `crates/crs/**` are
outside its assigned write scope. No frozen contract or production code needs changing.

1. `crates/readers/tests/shapefile_corpus.rs`, inside the existing case-name match, add:

```rust
"shp_corrupt_count_mismatch" => {
    let md = result.unwrap();
    assert_eq!(md.partial, [".shx indexes 2 shapes but .dbf has 1 records"]);
    assert_eq!(md.layers[0].feature_count, Some(2));
    assert!(md.layers[0].native_bounds.is_some());
},
"shp_cp437_names" => {
    let md = result.unwrap();
    assert_eq!(md.partial.len(), 1);
    assert!(md.partial[0].contains("CP437"));
    assert!(md.partial[0].contains("Windows-1252"));
    assert_eq!(md.layers[0].fields[1].name, "caf‚");
},
```

The current test rejects every unlisted `corrupt` case before checking its actual outcome.
The new data deliberately has SHX count2 and DBF count1; native extraction correctly reports
Partial and retains SHX count2. Keep the <64KiB assertion and all existing corrupt expectations.
The CP437 case verifies D-42/#42's existing unsupported-DOS fallback: GDAL has `café`, native
has `caf‚` with an explicit Partial diagnostic. The focused oracle test records this known
disagreement rather than claiming field-name agreement or changing the reader policy.

2. `crates/crs/tests/g2.rs`, prepend this entry to `KNOWN_CORPUS_SKIPS`:

```rust
("corpus layer with no raw CRS text and no GDAL EPSG code", 1),
```

The new `tif_custom_lcc_geokeys` stores binary user-defined GeoKeys, so the older text-only
G2 report has no raw text/direct EPSG to read. The shared extractor oracle **does** parse those
GeoKeys and checks the WGS84 result successfully. Keep every other skip, disagreement and rate
assertion unchanged. This is an explicit addition to a pinned inventory, not a CRS regression.

After applying both, rerun the full workspace tests and require fresh Windows/Linux CI.
