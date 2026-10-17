//! Small numeric helpers the measurement loop and report share.

/// The `p` quantile (0.0..=1.0) of `values` by nearest rank, or `None` for
/// no values. Sorts a copy, so callers can pass samples in arrival order.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted.get(rank).copied()
}

pub use bench_tools::density::spread;

/// Milliseconds, as the report keeps latencies.
pub fn millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_by_nearest_rank() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[7.0], 0.99), Some(7.0));
        let values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        assert_eq!(percentile(&values, 0.0), Some(1.0));
        assert_eq!(percentile(&values, 0.5), Some(51.0));
        assert_eq!(percentile(&values, 0.99), Some(99.0));
        assert_eq!(percentile(&values, 1.0), Some(100.0));
    }

    #[test]
    fn spread_covers_the_whole_range() {
        assert!(spread(0, 10).is_empty());
        assert!(spread(10, 0).is_empty());
        assert_eq!(spread(3, 10), vec![0, 1, 2]);
        assert_eq!(spread(100, 4), vec![0, 33, 66, 99]);
        let picks = spread(1000, 50);
        assert_eq!(picks.len(), 50);
        assert_eq!(picks.first(), Some(&0));
        assert_eq!(picks.last(), Some(&999));
        assert!(picks.windows(2).all(|w| w[0] < w[1]));
    }
}
