//! Shared reader → CRS → WGS84 path, including a bounded per-worker CRS cache.

use std::cell::RefCell;
use std::collections::HashMap;

use whereabouts_core::{
    CrsDefinition, DatasetMetadata, ExtractionStatus, LayerRecord, ReadError, ReadLimits,
    ResolvedCrs, ResolvedDataset,
};
use whereabouts_crs::{AxisPolicy, Transformer};

/// Reader facts enriched with resolved CRS and WGS84 bounds.
#[derive(Debug, Clone)]
pub struct ExtractedMetadata {
    /// Final status, including CRS and transformation limitations.
    pub status: ExtractionStatus,
    /// Layers in deterministic reader order.
    pub layers: Vec<LayerRecord>,
    /// An I/O or missing-member failure should be retried; keep any stored good record.
    pub retry: bool,
}

#[derive(Debug)]
struct CachedCrs {
    resolved: ResolvedCrs,
    transformer: Option<Transformer>,
    problem: Option<String>,
}

thread_local! {
    // HashMap compares the entire definition on hash collision. Limit both entries and key
    // bytes: a file may contain many distinct, very large definitions.
    static CACHE: RefCell<HashMap<CrsDefinition, CachedCrs>> = RefCell::new(HashMap::new());
}

/// Extracts a resolved dataset through the native reader and CRS pipeline.
///
/// This is the same entry point used by workers and the oracle (stage01/4.3). Reader failures
/// are status values, never panics. CRS failures retain native facts and produce `partial`.
/// Resolution and prepared transformations are cached per definition on each worker thread.
pub fn extract(dataset: &ResolvedDataset, limits: &ReadLimits) -> ExtractedMetadata {
    match whereabouts_readers::read(dataset, limits) {
        Ok(metadata) => enrich(metadata),
        Err(error) => ExtractedMetadata {
            status: error.status(),
            layers: Vec::new(),
            retry: matches!(error, ReadError::Io { .. } | ReadError::MissingMember(_)),
        },
    }
}

fn prepare(definition: &CrsDefinition) -> CachedCrs {
    let resolution = whereabouts_crs::resolve(definition);
    let mut problem = resolution.error.map(|e| e.to_string());
    let transformer = resolution.crs.as_ref().and_then(|crs| {
        match Transformer::new(crs, AxisPolicy::TraditionalGis) {
            Ok(t) => Some(t),
            Err(e) => {
                problem = Some(e.to_string());
                None
            }
        }
    });
    CachedCrs {
        resolved: resolution.resolved,
        transformer,
        problem,
    }
}

fn enrich(metadata: DatasetMetadata) -> ExtractedMetadata {
    let mut problems = metadata.partial;
    let mut layers = Vec::with_capacity(metadata.layers.len());
    for metadata in metadata.layers {
        let mut layer = LayerRecord {
            metadata,
            crs: None,
            wgs84: None,
        };
        if let Some(definition) = &layer.metadata.crs {
            CACHE.with_borrow_mut(|cache| {
                if !cache.contains_key(definition) {
                    let bytes: usize = cache.keys().map(|d| d.text.len()).sum();
                    if cache.len() >= 128 || bytes.saturating_add(definition.text.len()) > 1 << 20 {
                        cache.clear();
                    }
                    // Oversized definitions are processed, but never retained in the cache.
                    if definition.text.len() <= 1 << 20 {
                        cache.insert(definition.clone(), prepare(definition));
                    }
                }
                let uncached;
                let cached = match cache.get(definition) {
                    Some(c) => c,
                    None => {
                        uncached = prepare(definition);
                        &uncached
                    }
                };
                layer.crs = Some(cached.resolved.clone());
                if let Some(problem) = &cached.problem {
                    problems.push(format!("{}: CRS: {problem}", layer.metadata.name));
                }
                if let Some(bounds) = &layer.metadata.native_bounds {
                    match &cached.transformer {
                        Some(t) => match t.transform_bounds(bounds, 21) {
                            Ok(transformed) => {
                                layer.wgs84 = Some(transformed.extent);
                                if let Some(reason) = transformed.partial {
                                    problems.push(format!("{}: {reason}", layer.metadata.name));
                                }
                            }
                            Err(e) => problems.push(format!("{}: {e}", layer.metadata.name)),
                        },
                        None if cached.problem.is_none() => problems.push(format!(
                            "{}: CRS has no usable horizontal transform",
                            layer.metadata.name
                        )),
                        None => {}
                    }
                }
            });
        } else if layer.metadata.native_bounds.is_some() {
            problems.push(format!("{}: missing CRS", layer.metadata.name));
        }
        layers.push(layer);
    }
    problems.sort();
    problems.dedup();
    ExtractedMetadata {
        status: if problems.is_empty() {
            ExtractionStatus::Ok
        } else {
            ExtractionStatus::Partial(problems.join("; "))
        },
        layers,
        retry: false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use whereabouts_core::{Bounds, CrsDefinitionKind, LayerMetadata};

    #[test]
    fn resolves_transforms_and_preserves_unusable_definitions() {
        let mut md = DatasetMetadata::default();
        md.layers.push(LayerMetadata {
            name: "valid".into(),
            crs: Some(CrsDefinition::epsg(4326)),
            native_bounds: Some(Bounds::new(10., 20., 11., 21.).unwrap()),
            ..Default::default()
        });
        md.layers.push(LayerMetadata {
            name: "bad".into(),
            crs: Some(CrsDefinition::new(CrsDefinitionKind::Wkt1, "bad")),
            ..Default::default()
        });
        for _ in 0..2 {
            let result = enrich(md.clone());
            assert!(matches!(result.status, ExtractionStatus::Partial(_)));
            assert_eq!(result.layers[0].crs.as_ref().unwrap().epsg(), Some(4326));
            assert!(result.layers[0].wgs84.is_some());
            assert_eq!(result.layers[1].crs, Some(ResolvedCrs::Unparsed));
            for layer in result.layers {
                layer.validate().unwrap();
            }
        }
    }
}
