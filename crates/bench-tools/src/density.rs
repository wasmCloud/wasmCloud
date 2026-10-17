//! Sampling and serving evidence shared by both density benchmarks.

/// A capacity claim needs a positive observation and no failed requests.
/// A zero-request step is unmeasured, even if all workload starts succeeded.
pub fn served_sample(successes: usize, failures: usize, warmup_failures: usize) -> bool {
    successes > 0 && failures == 0 && warmup_failures == 0
}

/// Validate the sample size before allocating workloads.
pub fn sample_size(value: usize) -> Result<usize, &'static str> {
    if value == 0 {
        Err("sample size must be at least 1 to measure served capacity")
    } else {
        Ok(value)
    }
}

/// Distinct, ascending positions spanning the oldest and newest workloads.
/// A single observation targets the newest workload, where start failures are
/// most likely to be hidden by sampling only an already healthy older batch.
pub fn spread(count: usize, requested: usize) -> Vec<usize> {
    let n = requested.min(count);
    match n {
        0 => Vec::new(),
        1 => vec![count - 1],
        _ => (0..n)
            .map(|i| ((i as u128 * (count - 1) as u128) / (n - 1) as u128) as usize)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmeasured_or_failed_steps_are_not_served_capacity() {
        assert!(sample_size(0).is_err());
        assert_eq!(sample_size(1), Ok(1));
        assert!(!served_sample(0, 0, 0));
        assert!(!served_sample(1, 1, 0));
        assert!(!served_sample(100, 0, 1));
        assert!(served_sample(1, 0, 0));
    }

    #[test]
    fn samples_reach_the_newest_workload_without_overflow() {
        assert!(spread(0, 5).is_empty());
        assert!(spread(5, 0).is_empty());
        assert_eq!(spread(100, 1), vec![99]);
        assert_eq!(spread(100, 4), vec![0, 33, 66, 99]);
        assert_eq!(spread(3, 10), vec![0, 1, 2]);
        let large = spread(usize::MAX, 100);
        assert_eq!(large.first(), Some(&0));
        assert_eq!(large.last(), Some(&(usize::MAX - 1)));
        assert!(large.windows(2).all(|pair| matches!(pair, [a, b] if a < b)));
    }
}
