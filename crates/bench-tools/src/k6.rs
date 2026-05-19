//! Parser for k6's end-of-test summary JSON.
//!
//! `handleSummary(data)` in our k6 scripts writes `summary.json`.
//! k6's summary is a single JSON document with two pieces we care about:
//!
//! ```text
//! {
//!   "options": {
//!     "scenarios": {
//!       "<name>": { "tags": { "stage": "<stage>" }, … }
//!     }
//!   },
//!   "metrics": {
//!     "http_reqs":         { "values": { "rate":  <req/s>,    "count": … } },
//!     "http_req_duration": { "values": { "p(50)": <ms>, "p(95)": <ms>, "p(99)": <ms>, … } },
//!     "http_req_failed":   { "values": { "rate":  <0..1>,     "passes": …, "fails": … } }
//!   }
//! }
//! ```
//!
//! We emit one [`Row`] per `(scenario, stage, metric)` tuple. For a
//! single-scenario script the cardinality is `1 scenario × 1 stage × 5
//! metrics = 5 rows`; multi-scenario scripts (not currently used) would
//! produce `N × 1 × 5` rows. Per-scenario metric attribution is
//! single-scenario-only today — k6's global metrics don't break down by
//! scenario without explicit sub-metric tagging, so a future
//! multi-scenario script will need its own `handleSummary` that emits
//! per-scenario summary blocks. We bail with a clear error rather than
//! silently mis-attributing the global numbers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Debug)]
pub struct Row {
    /// k6 scenario name (e.g. `constant_rps`). Lands in `group`.
    pub group: String,
    /// Stage descriptor from the scenario's `tags.stage` (e.g. `200rps`).
    /// Lands in `param`. Falls back to `"default"` when the script forgot
    /// to tag — preserves a row at the cost of less informative naming.
    pub param: String,
    /// One of: `req_per_s`, `p50_ms`, `p95_ms`, `p99_ms`, `error_rate`.
    pub metric: &'static str,
    pub value: f64,
}

#[derive(Debug, Deserialize)]
struct Summary {
    options: Options,
    metrics: Metrics,
}

#[derive(Debug, Deserialize)]
struct Options {
    scenarios: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Metrics {
    #[serde(rename = "http_reqs")]
    http_reqs: Option<MetricBlock>,
    #[serde(rename = "http_req_duration")]
    http_req_duration: Option<MetricBlock>,
    #[serde(rename = "http_req_failed")]
    http_req_failed: Option<MetricBlock>,
}

#[derive(Debug, Deserialize)]
struct MetricBlock {
    values: serde_json::Map<String, serde_json::Value>,
}

/// Walk the k6 output dir and return one row per metric we know how to
/// surface. Returns an empty vec if the dir or `summary.json` is missing
/// — the caller (jsonl/summary subcommand) handles the empty case with a
/// human-readable warning rather than a hard error so a partial run still
/// uploads its log file to S3 instead of crashing the workflow.
pub fn walk(k6_dir: &Path) -> Result<Vec<Row>> {
    let path = k6_dir.join("summary.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let body = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let summary: Summary =
        serde_json::from_str(&body).with_context(|| format!("parse {}", path.display()))?;

    if summary.options.scenarios.is_empty() {
        bail!(
            "k6 summary at {} has no scenarios — script may not have set options.scenarios",
            path.display()
        );
    }
    if summary.options.scenarios.len() > 1 {
        // See module docstring: global metrics can't be safely attributed
        // to one of multiple scenarios. The fix is per-scenario tagged
        // sub-metrics in the script's handleSummary; that's a script-side
        // change, not something this parser should paper over.
        bail!(
            "k6 summary at {} has {} scenarios; parser only supports single-scenario scripts \
             (per-scenario attribution requires tagged sub-metrics — see crates/bench-tools/src/k6.rs)",
            path.display(),
            summary.options.scenarios.len()
        );
    }

    // Single entry — pull (name, stage) out of the scenarios map.
    let (scenario_name, scenario) = summary
        .options
        .scenarios
        .iter()
        .next()
        .ok_or_else(|| anyhow!("scenarios map asserted non-empty above"))?;
    let stage = scenario
        .get("tags")
        .and_then(|t| t.get("stage"))
        .and_then(|s| s.as_str())
        .unwrap_or("default")
        .to_string();

    let mut rows = Vec::with_capacity(5);
    let group = scenario_name.clone();

    if let Some(req_per_s) = summary
        .metrics
        .http_reqs
        .as_ref()
        .and_then(|m| number(&m.values, "rate"))
    {
        rows.push(Row {
            group: group.clone(),
            param: stage.clone(),
            metric: "req_per_s",
            value: req_per_s,
        });
    }

    // Latency percentiles. k6 reports `http_req_duration` in
    // milliseconds (its default time unit), which matches the metric
    // names we emit (`p50_ms` etc.) — no unit conversion needed.
    if let Some(dur) = summary.metrics.http_req_duration.as_ref() {
        for (key, metric) in [
            ("p(50)", "p50_ms"),
            ("p(95)", "p95_ms"),
            ("p(99)", "p99_ms"),
        ] {
            if let Some(v) = number(&dur.values, key) {
                rows.push(Row {
                    group: group.clone(),
                    param: stage.clone(),
                    metric,
                    value: v,
                });
            }
        }
    }

    if let Some(rate) = summary
        .metrics
        .http_req_failed
        .as_ref()
        .and_then(|m| number(&m.values, "rate"))
    {
        rows.push(Row {
            group,
            param: stage,
            metric: "error_rate",
            value: rate,
        });
    }

    if rows.is_empty() {
        eprintln!(
            "bench-tools k6: parsed {} but found no http_reqs / http_req_duration / http_req_failed values — \
             k6 script may not have issued any HTTP requests, or the summary schema changed",
            path.display(),
        );
    }

    Ok(rows)
}

fn number(values: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    values.get(key).and_then(|v| v.as_f64())
}

pub fn dir_from_target(target_dir: &Path) -> PathBuf {
    target_dir.join("k6")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_summary(tmp: &Path, body: &str) -> PathBuf {
        let dir = tmp.join("k6");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("summary.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        tmp.to_path_buf()
    }

    #[test]
    fn parses_single_scenario_summary() {
        let tmp = tempdir();
        let target = write_summary(
            &tmp,
            r#"{
                "options": {
                    "scenarios": {
                        "constant_rps": { "tags": { "stage": "200rps" } }
                    }
                },
                "metrics": {
                    "http_reqs":         { "values": { "rate": 199.5, "count": 5985 } },
                    "http_req_duration": { "values": { "p(50)": 1.2, "p(95)": 2.4, "p(99)": 5.6 } },
                    "http_req_failed":   { "values": { "rate": 0.0,  "passes": 5985, "fails": 0 } }
                }
            }"#,
        );
        let rows = walk(&dir_from_target(&target)).unwrap();
        let by_metric: std::collections::BTreeMap<_, _> =
            rows.iter().map(|r| (r.metric, r.value)).collect();
        assert_eq!(by_metric["req_per_s"], 199.5);
        assert_eq!(by_metric["p50_ms"], 1.2);
        assert_eq!(by_metric["p95_ms"], 2.4);
        assert_eq!(by_metric["p99_ms"], 5.6);
        assert_eq!(by_metric["error_rate"], 0.0);
        for row in &rows {
            assert_eq!(row.group, "constant_rps");
            assert_eq!(row.param, "200rps");
        }
    }

    #[test]
    fn missing_summary_returns_empty() {
        let tmp = tempdir();
        let rows = walk(&dir_from_target(&tmp)).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn multi_scenario_is_a_hard_error() {
        let tmp = tempdir();
        let target = write_summary(
            &tmp,
            r#"{
                "options": {
                    "scenarios": {
                        "a": { "tags": { "stage": "x" } },
                        "b": { "tags": { "stage": "y" } }
                    }
                },
                "metrics": {}
            }"#,
        );
        let err = walk(&dir_from_target(&target)).unwrap_err();
        assert!(err.to_string().contains("single-scenario"));
    }

    #[test]
    fn missing_stage_tag_falls_back() {
        let tmp = tempdir();
        let target = write_summary(
            &tmp,
            r#"{
                "options": { "scenarios": { "ramp": {} } },
                "metrics": {
                    "http_reqs": { "values": { "rate": 100.0 } }
                }
            }"#,
        );
        let rows = walk(&dir_from_target(&target)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].param, "default");
        assert_eq!(rows[0].group, "ramp");
    }

    fn tempdir() -> PathBuf {
        // Avoid an extra dep — std doesn't expose tempdir, but we can
        // synthesize a per-test directory under env::temp_dir() with
        // process pid + a counter. cargo test runs in parallel, so
        // include the test thread name to keep these unique.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let name = format!(
            "bench-tools-k6-test-{}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("anon"),
            n,
        );
        let p = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
