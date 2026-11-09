//! Opt-in query phase diagnostics; stdout remains reserved for query results.

use std::io::Write;
use std::time::Instant;

pub(crate) struct QueryTimings {
    start: Option<Instant>,
    last: Option<Instant>,
    phases: serde_json::Map<String, serde_json::Value>,
}

impl QueryTimings {
    pub(crate) fn new(enabled: bool) -> Self {
        let start = enabled.then(Instant::now);
        Self {
            start,
            last: start,
            phases: serde_json::Map::new(),
        }
    }

    pub(crate) fn mark(&mut self, phase: &str) {
        if let Some(last) = self.last {
            let now = Instant::now();
            self.phases.insert(
                phase.into(),
                (now.duration_since(last).as_secs_f64() * 1000.).into(),
            );
            self.last = Some(now);
        }
    }

    pub(crate) fn finish(self) -> anyhow::Result<()> {
        if let (Some(start), Some(last)) = (self.start, self.last) {
            let report = serde_json::json!({
                "kind": "query_timings",
                "phases_ms": self.phases,
                "query_total_ms": last.duration_since(start).as_secs_f64() * 1000.,
            });
            let mut stderr = std::io::stderr().lock();
            serde_json::to_writer(&mut stderr, &report)?;
            writeln!(stderr)?;
        }
        Ok(())
    }
}
