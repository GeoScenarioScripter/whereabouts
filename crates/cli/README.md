# Whereabouts CLI

```console
whereabouts index --volume X: --db catalog.db
whereabouts inspect X:\data\roads.shp
whereabouts inspect X:\data\roads.prj --json
whereabouts query roads --db catalog.db --format shp --epsg 4326 --bbox 170,-20,-170,20 --json
```

`index` requires an elevated Windows shell. `inspect` and `query` do not require
volume access or elevation. Query reads the catalog and `<db>.names`; it does not
refresh the index. Run `index` again to pick up changes. A snapshot with a missing,
corrupt, ambiguous, or mismatched checkpoint fails with a diagnostic rather than
mixing states. Checkpoint selection, filters, and result records share one catalog read snapshot,
so concurrent source updates and metadata-only retries cannot mix database states within a query.
For a catalog with multiple sources, provide each source's distinct `--snapshot`
and, if checkpoint matching is ambiguous, `--volume-id` (the catalog's numeric ID).

Query text is a case-insensitive literal filename substring. Empty text returns
all matching datasets. Operators and wildcard syntax are deferred. Members such
as `.prj` and `.dbf` match their owning dataset, once per dataset. Inaccessible
snapshot paths (orphans or cycles) are hidden, even when a sidecar is accessible.
Ordering is exact > prefix > substring against the best matching member name or
dataset display stem, then newest member modification time, then stable dataset ID.
`--limit` defaults to 100 and applies after ranking. `--format` accepts core codes
such as `shp`, `gpkg`, `gtiff`, separated by commas or repeated. EPSG and bbox must
match the same layer; west greater than east crosses the antimeridian.

Inspect groups a path with its siblings and extracts through the same native
reader/CRS pipeline as indexing. A member path can resolve to multiple datasets
(for example a shared `.prj`). Paths directly inside a recognized `.gdb` resolve to that folder dataset.
The requested path is canonicalized; directory discovery is capped at 100,000 entries,
does not recurse, and skips symlink entries;
readers retain their normal byte/time budgets. No index, catalog, or transaction
sidecars are created. Inspect JSON IDs are temporary grouping IDs (volume 0),
not catalog identities. A malformed dataset is reported with an `error` status;
command failures such as a missing path or an orphan sidecar return a nonzero exit.

Both commands emit a JSON array of complete dataset records with `--json`.
Without it, query emits a tab-separated dataset table; inspect adds layer CRS,
native/WGS84 bounds and raster metadata. Control characters are escaped in tables.
Diagnostics and index progress go to stderr.

Repeatable synthetic startup and reused-query timing:

```console
cargo test --release -p whereabouts-cli --test startup -- --ignored --nocapture
```

This measures 1,000,010 names, five catalog datasets (one orphan hidden), ten
runs per term and the time to the first **dataset row**, excluding the header.
It is a warm-cache process startup measurement, not real-volume indexing evidence.

To measure candidate-dataset scaling separately (10,000 and 100,000 one-layer datasets):

```console
cargo test --release -p whereabouts-cli --test startup query_at_gis_scale -- --ignored --nocapture
```
