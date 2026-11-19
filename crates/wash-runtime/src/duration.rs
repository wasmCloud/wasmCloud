//! Human-readable durations used by host configuration and the CLI.

use std::time::Duration;

use anyhow::{Context as _, bail, ensure};

/// Parse a nonnegative duration such as `30s`, `1m30s`, or `500ms`.
///
/// Units range from nanoseconds to years. Months are 30.44 days and years are
/// 365.25 days. Components may repeat or appear in any order. A bare `0` is
/// accepted; other numbers need a unit. Fractions must be exact to a nanosecond
/// for minutes and smaller units, or a second for hours and larger units.
pub fn parse_duration(input: &str) -> anyhow::Result<Duration> {
    if input == "0" {
        return Ok(Duration::ZERO);
    }

    let mut remaining = input.trim();
    ensure!(!remaining.is_empty(), "duration is empty");
    let mut total = 0_u128;

    while !remaining.is_empty() {
        let number_end = remaining
            .find(|character: char| !character.is_ascii_digit() && !character.is_whitespace())
            .unwrap_or(remaining.len());
        let (integer, tail) = remaining.split_at(number_end);
        let integer = integer
            .split_whitespace()
            .collect::<String>()
            .parse::<u64>()
            .context("invalid duration number")?;

        let (fraction, tail) = if let Some(tail) = tail.strip_prefix('.') {
            let fraction_end = tail
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(tail.len());
            let (digits, tail) = tail.split_at(fraction_end);
            let numerator = digits.parse::<u64>().context("invalid duration fraction")?;
            let digits = u32::try_from(digits.len()).context("duration fraction is too precise")?;
            let denominator = 10_u64
                .checked_pow(digits)
                .context("duration fraction is too precise")?;
            (Some((numerator, denominator)), tail)
        } else {
            (None, tail)
        };

        let tail = tail.trim_start();
        let unit_end = tail
            .find(|character: char| !character.is_ascii_alphabetic() && character != 'µ')
            .unwrap_or(tail.len());
        let (unit, tail) = tail.split_at(unit_end);
        let unit_nanos: u128 = match unit {
            "nanos" | "nsec" | "ns" => 1,
            "usec" | "us" | "µs" => 1_000,
            "millis" | "msec" | "ms" => 1_000_000,
            "seconds" | "second" | "secs" | "sec" | "s" => 1_000_000_000,
            "minutes" | "minute" | "min" | "mins" | "m" => 60_000_000_000,
            "hours" | "hour" | "hr" | "hrs" | "h" => 3_600_000_000_000,
            "days" | "day" | "d" => 86_400_000_000_000,
            "weeks" | "week" | "wk" | "wks" | "w" => 604_800_000_000_000,
            "months" | "month" | "M" => 2_630_016_000_000_000,
            "years" | "year" | "yr" | "yrs" | "y" => 31_557_600_000_000_000,
            "" => bail!("duration needs a unit (e.g. `5s` or `500ms`)"),
            _ => bail!("unknown duration unit `{unit}`"),
        };

        let mut nanos = u128::from(integer) * unit_nanos;
        if let Some((numerator, denominator)) = fraction {
            ensure!(unit_nanos != 1, "fractional nanoseconds are not supported");
            let resolution = if unit_nanos >= 3_600_000_000_000 {
                1_000_000_000
            } else {
                1
            };
            let numerator = u128::from(numerator) * (unit_nanos / resolution);
            let denominator = u128::from(denominator);
            ensure!(
                numerator.is_multiple_of(denominator),
                "duration fraction loses precision"
            );
            nanos += (numerator / denominator) * resolution;
        }
        total = total.checked_add(nanos).context("duration is too large")?;
        remaining = tail.trim_start();
    }

    let seconds = u64::try_from(total / 1_000_000_000).context("duration is too large")?;
    let nanos = u32::try_from(total % 1_000_000_000).context("invalid duration precision")?;
    Ok(Duration::new(seconds, nanos))
}

#[cfg(test)]
mod tests {
    use super::parse_duration;
    use std::time::Duration;

    #[test]
    fn configuration_durations_keep_their_units_and_precision() {
        for (input, expected) in [
            ("0", Duration::ZERO),
            ("500ms", Duration::from_millis(500)),
            ("1m30s", Duration::from_secs(90)),
            ("1.5h 30m", Duration::from_secs(7_200)),
            ("30m 1h", Duration::from_secs(5_400)),
            ("1h 1h", Duration::from_secs(7_200)),
            ("0.5d", Duration::from_secs(43_200)),
            ("1y 1M", Duration::from_secs(34_187_616)),
            ("1.000000001s", Duration::new(1, 1)),
            ("1 0s", Duration::from_secs(10)),
            ("1 .5s", Duration::from_millis(1_500)),
            ("18446744073709551615s 999999999ns", Duration::MAX),
        ] {
            assert_eq!(parse_duration(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn invalid_or_overflowing_durations_are_rejected() {
        for input in [
            "",
            " ",
            "5",
            "0 ",
            "-5s",
            "+5s",
            "1e3s",
            "1.s",
            "1.5ns",
            "0.001d",
            "0.0001h",
            "0.0000000001s",
            "1s,1h",
            "1fortnight",
            "18446744073709551616s",
            "18446744073709551615s 1s",
        ] {
            assert!(parse_duration(input).is_err(), "{input}");
        }
    }
}
