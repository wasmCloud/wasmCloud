pub mod blobstore;
pub mod http;
pub mod keyvalue;
pub mod logging;
pub mod messaging;
pub mod numbergen;

pub use self::http::{
    ClientRequest as HttpClientRequest, Response as HttpResponse,
    ServerRequest as HttpServerRequest,
};

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Error returned if time valid provided is invalid
pub const INVALID_DATETIME: &str = "Invalid DateTime";

/// Timestamp - represents absolute time in UTC,
/// as non-leap seconds and nanoseconds since the UNIX EPOCH
/// It is recommended to use the `new` constructor to check parameters for validity
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Timestamp {
    /// The number of non-leap seconds since UNIX EPOCH in UTC
    pub sec: i64,
    /// The number of nanoseconds since the beginning of the last whole non-leap second
    pub nsec: u32,
}

impl Timestamp {
    /// Constructs a timestamp, with parameter validation
    pub fn new(sec: i64, nsec: u32) -> Result<Self, &'static str> {
        if sec > 0 && (sec < i64::MAX / 1_000_000_001) && nsec < 1_000_000_000 {
            Ok(Self { sec, nsec })
        } else {
            Err(INVALID_DATETIME)
        }
    }

    /// constructs a time stamp from the current system time UTC
    /// See [SystemTime](https://doc.rust-lang.org/std/time/struct.SystemTime.html) for platform implementations
    pub fn now() -> Timestamp {
        SystemTime::now().into()
    }

    /// Returns time in nanoseconds
    pub fn as_nanos(&self) -> u128 {
        (self.sec as u128 * 1_000_000_000) + self.nsec as u128
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
            nsec: d.subsec_nanos(),
        }
    }
}

impl TryInto<SystemTime> for Timestamp {
    type Error = &'static str;

    fn try_into(self) -> Result<SystemTime, Self::Error> {
        use std::time::Duration;

        let sys_now = SystemTime::now();
        let now = Self::from(sys_now).as_nanos();
        let then = self.as_nanos();
        if now >= then {
            let delta_past = now - then;
            sys_now
                .checked_sub(Duration::from_nanos(delta_past as u64))
                .ok_or(INVALID_DATETIME)
        } else {
            let delta_fut = then - now; // future time
            if delta_fut > i64::MAX as u128 {
                Err(INVALID_DATETIME)
            } else {
                sys_now
                    .checked_add(Duration::from_nanos(delta_fut as u64))
                    .ok_or(INVALID_DATETIME)
            }
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
fn timestamp_system_time() {
    // past
    let st = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(86_400 * 365 * 5))
        .unwrap();
    let t = Timestamp::from(st);
    assert_eq!(t.try_into(), Ok(st));

    // future
    let st = SystemTime::now()
        .checked_add(std::time::Duration::from_secs(86_400 * 365))
        .unwrap();
    let t = Timestamp::from(st);
    let st_check: SystemTime = t.try_into().unwrap();
    assert_eq!(st_check, st);
}

#[test]
fn timestamp_default() {
    let t1 = Timestamp::default();
    let t2 = Timestamp::now();

    assert!(t1 <= t2);

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
    assert_ne!(t1, t4);
}
