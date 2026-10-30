//! Shared formatting helpers for markdown tables.
//!
//! Per-row metric semantics mirror the trend site (`arewefastyet`): use
//! req/s for batch throughput (`Throughput::Elements > 1`), bytes/sec for
//! byte throughput, and time otherwise. Keep this in sync if the site's
//! display rules change.

use crate::criterion::{Row, Throughput};

#[derive(Debug, Clone, Copy)]
pub enum Unit {
    Time,
    Rps,
    Bps,
}

impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Unit::Time => "time",
            Unit::Rps => "req/s",
            Unit::Bps => "B/s",
        }
    }
}

/// Decide the natural display unit for a row and translate the criterion
/// time-based estimates (in ns) into that unit. Returns the unit plus the
/// converted (mean, median, std_dev, ci_low, ci_high) tuple.
pub fn metric(row: &Row) -> (Unit, MetricValues) {
    let mean_ns = row.estimates.mean.point_estimate;
    let median_ns = row.estimates.median.point_estimate;
    let std_dev_ns = row.estimates.std_dev.point_estimate;
    let ci_low_ns = row.estimates.mean.confidence_interval.lower_bound;
    let ci_high_ns = row.estimates.mean.confidence_interval.upper_bound;

    match row.throughput {
        Some(Throughput::Elements(n)) if n > 1 => {
            let k = (n as f64) * 1e9;
            (
                Unit::Rps,
                MetricValues {
                    // rate = k / time; lower time ↔ higher rate, so CI inverts.
                    mean: k / mean_ns,
                    median: k / median_ns,
                    // Approximate std-dev in rate units by linearising around mean.
                    std_dev: (k / mean_ns) - (k / (mean_ns + std_dev_ns)),
                    ci_low: k / ci_high_ns,
                    ci_high: k / ci_low_ns,
                },
            )
        }
        Some(Throughput::Bytes(n) | Throughput::BytesDecimal(n)) if n > 0 => {
            let k = (n as f64) * 1e9;
            (
                Unit::Bps,
                MetricValues {
                    mean: k / mean_ns,
                    median: k / median_ns,
                    std_dev: (k / mean_ns) - (k / (mean_ns + std_dev_ns)),
                    ci_low: k / ci_high_ns,
                    ci_high: k / ci_low_ns,
                },
            )
        }
        _ => (
            Unit::Time,
            MetricValues {
                mean: mean_ns,
                median: median_ns,
                std_dev: std_dev_ns,
                ci_low: ci_low_ns,
                ci_high: ci_high_ns,
            },
        ),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MetricValues {
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
    pub ci_low: f64,
    pub ci_high: f64,
}

/// Format a value into the given unit at human-friendly scale.
/// Mirrors the unit scaling used on the `arewefastyet` trend site so
/// values rendered into `$GITHUB_STEP_SUMMARY` match what the dashboard
/// displays for the same row.
pub fn fmt(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Time => fmt_time(v),
        Unit::Rps => fmt_rps(v),
        Unit::Bps => fmt_bps(v),
    }
}

fn fmt_time(ns: f64) -> String {
    if ns < 1_000.0 {
        format!("{} ns", ns.floor() as i64)
    } else if ns < 1_000_000.0 {
        format!("{:.2} µs", ns / 1_000.0)
    } else if ns < 1_000_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else {
        format!("{:.2} s", ns / 1_000_000_000.0)
    }
}

fn fmt_rps(rps: f64) -> String {
    if rps >= 1_000_000.0 {
        format!("{:.2} Mreq/s", rps / 1_000_000.0)
    } else if rps >= 1_000.0 {
        format!("{:.2} Kreq/s", rps / 1_000.0)
    } else {
        format!("{} req/s", rps.floor() as i64)
    }
}

fn fmt_bps(bps: f64) -> String {
    if bps >= 1_000_000_000.0 {
        format!("{:.2} GB/s", bps / 1_000_000_000.0)
    } else if bps >= 1_000_000.0 {
        format!("{:.2} MB/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.2} KB/s", bps / 1_000.0)
    } else {
        format!("{} B/s", bps.floor() as i64)
    }
}

/// `1_234_567` → `"1,234,567"`. Used by callers that want a human-readable
/// integer count (e.g. instruction totals in the gungraun summary).
pub fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod metric_tests {
    use super::*;
    use crate::criterion::{ConfidenceInterval, Estimate, Estimates};

    fn row(throughput: Option<Throughput>) -> Row {
        let estimate = |value, lower, upper| Estimate {
            point_estimate: value,
            confidence_interval: ConfidenceInterval {
                lower_bound: lower,
                upper_bound: upper,
            },
        };
        Row {
            group: "group".into(),
            param: "param".into(),
            throughput,
            estimates: Estimates {
                mean: estimate(1e9, 8e8, 1.2e9),
                median: estimate(5e8, 4e8, 6e8),
                std_dev: estimate(1e8, 9e7, 1.1e8),
            },
        }
    }

    #[test]
    fn batched_elements_become_rates_and_invert_the_time_interval() {
        let (unit, values) = metric(&row(Some(Throughput::Elements(2))));
        assert!(matches!(unit, Unit::Rps));
        assert_eq!(unit.label(), "req/s");
        assert_eq!(values.mean, 2.0);
        assert_eq!(values.median, 4.0);
        assert!((values.std_dev * 11.0 - 2.0).abs() < 1e-12);
        assert!((values.ci_low - 5.0 / 3.0).abs() < 1e-12);
        assert_eq!(values.ci_high, 2.5);
        assert_eq!(fmt(values.mean, unit), "2 req/s");
    }

    #[test]
    fn binary_and_decimal_byte_counts_share_the_same_rate_conversion() {
        for throughput in [Throughput::Bytes(10), Throughput::BytesDecimal(10)] {
            let (unit, values) = metric(&row(Some(throughput)));
            assert!(matches!(unit, Unit::Bps));
            assert_eq!(unit.label(), "B/s");
            assert_eq!(values.mean, 10.0);
            assert_eq!(values.median, 20.0);
            assert!((values.std_dev * 11.0 - 10.0).abs() < 1e-12);
            assert!((values.ci_low - 25.0 / 3.0).abs() < 1e-12);
            assert_eq!(values.ci_high, 12.5);
            assert_eq!(fmt(values.mean, unit), "10 B/s");
        }
    }

    #[test]
    fn absent_or_non_batch_throughput_keeps_time_estimates() {
        for throughput in [
            None,
            Some(Throughput::Elements(0)),
            Some(Throughput::Elements(1)),
            Some(Throughput::Bytes(0)),
            Some(Throughput::BytesDecimal(0)),
        ] {
            let (unit, values) = metric(&row(throughput));
            assert!(matches!(unit, Unit::Time));
            assert_eq!(unit.label(), "time");
            assert_eq!(values.mean, 1e9);
            assert_eq!(values.median, 5e8);
            assert_eq!(values.std_dev, 1e8);
            assert_eq!(values.ci_low, 8e8);
            assert_eq!(values.ci_high, 1.2e9);
            assert_eq!(fmt(values.mean, unit), "1.00 s");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_scales() {
        assert_eq!(fmt_time(500.0), "500 ns");
        assert_eq!(fmt_time(1_500.0), "1.50 µs");
        assert_eq!(fmt_time(1_500_000.0), "1.50 ms");
        assert_eq!(fmt_time(1_500_000_000.0), "1.50 s");
    }

    #[test]
    fn rps_scales() {
        assert_eq!(fmt_rps(800.0), "800 req/s");
        assert_eq!(fmt_rps(1_500.0), "1.50 Kreq/s");
        assert_eq!(fmt_rps(1_500_000.0), "1.50 Mreq/s");
    }

    #[test]
    fn bps_scales() {
        assert_eq!(fmt_bps(800.0), "800 B/s");
        assert_eq!(fmt_bps(1_500.0), "1.50 KB/s");
        assert_eq!(fmt_bps(1_500_000.0), "1.50 MB/s");
        assert_eq!(fmt_bps(1_500_000_000.0), "1.50 GB/s");
    }

    #[test]
    fn thousands_formatter() {
        assert_eq!(fmt_thousands(0), "0");
        assert_eq!(fmt_thousands(999), "999");
        assert_eq!(fmt_thousands(1_000), "1,000");
        assert_eq!(fmt_thousands(1_234_567), "1,234,567");
        assert_eq!(fmt_thousands(12_345_678_901), "12,345,678,901");
    }
}
