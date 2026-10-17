//! Byte sizes as people write them (`4GiB`, `512M`, `1.5Gi`) and as the
//! report prints them.

use anyhow::{Result, bail};

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;
const TIB: u64 = GIB * 1024;

/// Parse a size: a number, optionally fractional, then an optional unit.
/// The `i` forms are binary and the bare letters decimal, as Kubernetes and
/// `wash host` spell them; `KiB`/`KB` style suffixes are accepted too.
pub fn parse(input: &str) -> Result<u64> {
    let s = input.trim();
    let end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(end);
    if number.is_empty() {
        bail!("{input:?} is not a size; write e.g. 4GiB or 512MiB");
    }
    let number: f64 = number
        .parse()
        .map_err(|_| anyhow::anyhow!("{input:?} is not a size; write e.g. 4GiB or 512MiB"))?;
    let factor = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1000,
        "ki" | "kib" => KIB,
        "m" | "mb" => 1000 * 1000,
        "mi" | "mib" => MIB,
        "g" | "gb" => 1000 * 1000 * 1000,
        "gi" | "gib" => GIB,
        "t" | "tb" => 1000 * 1000 * 1000 * 1000,
        "ti" | "tib" => TIB,
        other => bail!("{input:?} has an unknown unit {other:?}; use B, KiB, MiB, GiB or TiB"),
    };
    let bytes = number * factor as f64;
    if !bytes.is_finite() || bytes < 1.0 || bytes > u64::MAX as f64 {
        bail!("{input:?} is out of range");
    }
    Ok(bytes.round() as u64)
}

/// Render bytes with a binary unit and three significant-ish digits.
pub fn render(bytes: u64) -> String {
    let (value, unit) = match bytes {
        b if b >= TIB => (b as f64 / TIB as f64, "TiB"),
        b if b >= GIB => (b as f64 / GIB as f64, "GiB"),
        b if b >= MIB => (b as f64 / MIB as f64, "MiB"),
        b if b >= KIB => (b as f64 / KIB as f64, "KiB"),
        b => return format!("{b} B"),
    };
    if value >= 100.0 {
        format!("{value:.0} {unit}")
    } else if value >= 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.2} {unit}")
    }
}

/// [`render`], or `-` when the size is unknown.
pub fn render_opt(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "-".to_string(), render)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_binary_and_decimal_units() {
        assert_eq!(parse("4GiB").unwrap(), 4 * GIB);
        assert_eq!(parse("4Gi").unwrap(), 4 * GIB);
        assert_eq!(parse("512MiB").unwrap(), 512 * MIB);
        assert_eq!(parse("512 mib").unwrap(), 512 * MIB);
        assert_eq!(parse("1G").unwrap(), 1_000_000_000);
        assert_eq!(parse("1.5GiB").unwrap(), 3 * GIB / 2);
        assert_eq!(parse("1048576").unwrap(), MIB);
        assert_eq!(parse("2KiB").unwrap(), 2048);
        assert_eq!(parse(" 1TiB ").unwrap(), TIB);
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse("").is_err());
        assert!(parse("GiB").is_err());
        assert!(parse("4 parsecs").is_err());
        assert!(parse("0").is_err());
        assert!(parse("1.2.3G").is_err());
    }

    #[test]
    fn renders_readably() {
        assert_eq!(render(512), "512 B");
        assert_eq!(render(2048), "2.00 KiB");
        assert_eq!(render(15 * MIB), "15.0 MiB");
        assert_eq!(render(4 * GIB), "4.00 GiB");
        assert_eq!(render(300 * MIB), "300 MiB");
        assert_eq!(render_opt(None), "-");
    }
}
