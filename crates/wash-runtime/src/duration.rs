//! Human-readable durations used by host configuration and the CLI.

use std::time::Duration;

use anyhow::Context as _;
use jiff::{SpanRelativeTo, fmt::friendly::SpanParser};

/// Parse a nonnegative duration such as `30s`, `1m30s`, or `500ms`.
///
/// Uses Jiff's friendly format: units must appear from largest to smallest,
/// without repeats, and only the last unit may be fractional. A bare `0` is
/// accepted. Days and weeks are fixed multiples of 24 hours; months and years
/// are rejected because their length depends on a calendar date.
pub fn parse_duration(input: &str) -> anyhow::Result<Duration> {
    let input = input.trim();
    if input == "0" {
        return Ok(Duration::ZERO);
    }

    let parser = SpanParser::new();
    if let Ok(duration) = parser.parse_unsigned_duration(input) {
        return Ok(duration);
    }

    let span = parser.parse_span(input).context("invalid duration")?;
    let duration = span
        .to_duration(SpanRelativeTo::days_are_24_hours())
        .context("duration must use fixed-length units up to weeks")?;
    Duration::try_from(duration).context("duration must be nonnegative")
}

#[cfg(test)]
mod tests {
    use super::parse_duration;
    use std::time::Duration;

    #[test]
    fn configuration_durations_keep_their_units_and_precision() -> anyhow::Result<()> {
        for (input, expected) in [
            ("0", Duration::ZERO),
            (" 0 ", Duration::ZERO),
            ("500ms", Duration::from_millis(500)),
            ("1m30s", Duration::from_secs(90)),
            ("1h30m", Duration::from_secs(5_400)),
            ("1.5h", Duration::from_secs(5_400)),
            ("1d 12h", Duration::from_secs(129_600)),
            ("1w 1d", Duration::from_secs(691_200)),
            ("1.000000001s", Duration::new(1, 1)),
            ("1 hour, 30 minutes", Duration::from_secs(5_400)),
            ("0.0001h", Duration::from_millis(360)),
            ("18446744073709551615s 999999999ns", Duration::MAX),
        ] {
            assert_eq!(parse_duration(input)?, expected, "{input}");
        }
        Ok(())
    }

    #[test]
    fn invalid_or_overflowing_durations_are_rejected() {
        for input in [
            "",
            " ",
            "5",
            "-5s",
            "-1d",
            "1s ago",
            "1e3s",
            "1.s",
            "1.5ns",
            "0.0000000001s",
            "1fortnight",
            "18446744073709551616s",
            "18446744073709551615s 1s",
        ] {
            assert!(parse_duration(input).is_err(), "{input}");
        }
    }

    #[test]
    fn durations_follow_jiffs_unit_order_and_calendar_rules() {
        for input in [
            "30m 1h", "1h 1h", "1.5h 30m", "0.5d", "1M", "1month", "1y", "1 0s", "1 .5s",
        ] {
            assert!(parse_duration(input).is_err(), "{input}");
        }
    }
}
