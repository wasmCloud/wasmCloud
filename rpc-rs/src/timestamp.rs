//! Timestamp class - represents a Smithy Timestamp
//!
//! Timestamp is stored as the number of non-leap seconds
//! since January 1, 1970 0:00:00 UTC (aka "UNIX EPOCH")
//! and the number of nanoseconds since the last whole non-leap second.
//!
//! For converting to and from other formats such as ISO8601/ISO3339,
//! and to/from other timezones,
//! the [chrono](https://crates.io/crate/chrono) crate is recommended.
//!

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    convert::TryFrom,
    time::{SystemTime, UNIX_EPOCH},
};

/// Timestamp - represents absolute time in UTC,
/// as non-leap seconds and nanoseconds since the UNIX EPOCH
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Timestamp {
    /// The number of non-leap seconds since UNIX EPOCH in UTC
    pub sec: i64,
    /// The number of nanoseconds since the beginning of the last whole non-leap second
    pub nsec: u32,
}

impl Timestamp {
    /// constructs a time stamp from the current system time UTC
    /// See [SystemTime](https://doc.rust-lang.org/std/time/struct.SystemTime.html) for platform implementations
    pub fn now() -> Timestamp {
        SystemTime::now().into()
    }
}

impl Default for Timestamp {
    /// constructs a time stamp from the current system time UTC
    /// See [SystemTime](https://doc.rust-lang.org/std/time/struct.SystemTime.html) for platform implementations
    fn default() -> Timestamp {
        Self::now()
    }
}

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.sec.cmp(&other.sec) {
            Ordering::Equal => self.nsec.cmp(&other.nsec),
            ord => ord,
        }
    }
}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Error returned if time valid provided is invalid
pub const INVALID_DATETIME: &str = "Invalid DateTime";

impl TryFrom<Timestamp> for DateTime<Utc> {
    type Error = &'static str;

    ///
    /// Returns the DateTime, or an error if the parameter is invalid.
    fn try_from(ts: Timestamp) -> Result<DateTime<Utc>, Self::Error> {
        //) -> Option<chrono::DateTime<chrono::Timestamp::UTC>> {
        match NaiveDateTime::from_timestamp_opt(ts.sec, ts.nsec) {
            Some(dt) => Ok(Utc.from_utc_datetime(&dt)),
            None => Err(INVALID_DATETIME),
        }
    }
}

impl From<DateTime<Utc>> for Timestamp {
    /// Creates a Timestamp from chrono DateTime
    fn from(dt: DateTime<Utc>) -> Timestamp {
        const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;
        let nanos = dt.timestamp_nanos();
        Timestamp {
            sec: nanos / NANOSECONDS_PER_SECOND,
            nsec: (nanos % NANOSECONDS_PER_SECOND) as u32,
        }
    }
}

impl From<SystemTime> for Timestamp {
    /// Creates a Timestamp from rust's SystemTime
    fn from(st: SystemTime) -> Timestamp {
        let d = st
            .duration_since(UNIX_EPOCH)
            // library code shouldn't panic, but if the system time clock
            // thinks its _before_ the epoch, the system is having worse problems than this
            .expect("system time before Unix epoch");
        Timestamp {
            sec: d.as_secs() as i64,
            nsec: d.subsec_nanos() as u32,
        }
    }
}

#[test]
fn timestamp_updates() {
    let now = Timestamp::now();

    std::thread::sleep(std::time::Duration::from_nanos(5_000));

    let then = Timestamp::now();
    assert!(then.sec > now.sec || (then.sec == now.sec && then.nsec > now.nsec));
}

#[test]
fn timestamp_to_datetime() {
    use chrono::{DateTime, Utc};
    use std::convert::TryInto;

    let start: Timestamp = Timestamp {
        sec: 2_000_000_000,
        nsec: 100_000,
    };

    let dt: DateTime<Utc> = start.try_into().unwrap();

    let next: Timestamp = dt.into();

    assert_eq!(&start.sec, &next.sec);
    assert_eq!(&start.nsec, &next.nsec);
}

#[test]
fn timestamp_default() {
    let t1 = Timestamp::default();
    let t2 = Timestamp::now();

    assert!(t1 == t2 || t1 < t2);

    assert!(t1.sec > 1600000000);
    assert!(t1.sec < 3000000000);

    assert!(t2.sec > 1600000000);
    assert!(t2.sec < 3000000000);
}

#[test]
fn timestamp_ordering() {
    // equals
    let t1 = Timestamp {
        sec: 100,
        nsec: 100,
    };
    let t2 = Timestamp {
        sec: 100,
        nsec: 100,
    };
    assert_eq!(t1, t2);

    // if sec differs, ignore nsec
    let t3 = Timestamp { sec: 99, nsec: 400 };
    assert!(t1 > t3);
    let t3 = Timestamp { sec: 101, nsec: 40 };
    assert!(t1 < t3);

    // if sec same, use nsec
    let t4 = Timestamp {
        sec: 100,
        nsec: 400,
    };
    assert!(t1 < t4);
    let t4 = Timestamp { sec: 100, nsec: 40 };
    assert!(t1 > t4);

    // not equals
    assert!(t1 != t4);
}
