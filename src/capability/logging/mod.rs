/// A logging capability discarding all logging statements
pub mod discard;
/// [log](::log) crate adaptors for logging capability
pub mod log;

pub use self::discard::Logging as DiscardLogging;
pub use self::log::Logging as LogLogging;

use core::str::FromStr;

use anyhow::{bail, Context, Error, Result};
use wasmbus_rpc::common::deserialize;
use wasmcloud_interface_logging::LogEntry;

/// Logging invocation
#[derive(Clone, Debug)]
pub enum Invocation {
    /// Write a log record
    WriteLog {
        /// [Level] to log at
        level: Level,
        /// Text to log
        text: String,
    },
}

impl<O, P> TryFrom<(O, Option<P>)> for Invocation
where
    O: AsRef<str>,
    P: AsRef<[u8]>,
{
    type Error = Error;

    fn try_from((operation, payload): (O, Option<P>)) -> Result<Self> {
        match operation.as_ref() {
            "Logging.WriteLog" => {
                let payload = payload.context("payload cannot be empty")?;
                let LogEntry { level, text } =
                    deserialize(payload.as_ref()).context("failed to deserialize log entry")?;
                let level = level.parse().context("failed to parse log level")?;
                Ok(Invocation::WriteLog { level, text })
            }
            operation => bail!("unknown operation: `{operation}`"),
        }
    }
}

/// Logging verbosity level
#[derive(Copy, Clone, Debug)]
pub enum Level {
    /// Log at debug level
    Debug,
    /// Log at info level
    Info,
    /// Log at warn level
    Warn,
    /// Log at error level
    Error,
}

impl FromStr for Level {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "debug" => Ok(Level::Debug),
            "info" => Ok(Level::Info),
            "warn" => Ok(Level::Warn),
            "error" => Ok(Level::Error),
            level => bail!("unknown level `{level}`"),
        }
    }
}
