//! Simple logging for multi-threaded programs that sends all logs
//! through a crossbeam channel to a logging thread.
//! This works with the existing log macros (debug!, etc.), which
//! can be sent by any thread.
//!
//! The level can be set with log::set_level() or with RUST_LOG
//!
#![cfg(not(target_arch = "wasm32"))]

use log::{Level, Metadata, Record};
use once_cell::sync::OnceCell;
use std::{
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};

// Number of log messages that can be queued before the sender is blocked.
// There is a dedicated log thread  pulling from the queue so this number should
// be high enough to handle bursty writes on a platform with slow disk io.
// If it were necessary to have _no_ queuing of log messages, change this to 0,
// so that every log write will be synchronous with pulling from the queue.
const MESSAGE_LIMIT: usize = 50;

struct ChannelLogger {
    tx: crossbeam::channel::Sender<LogRec>,
}

/// Receiving end of the logging channel.
pub type Receiver = crossbeam::channel::Receiver<LogRec>;

// Storing the channel sender statically is what allows us to use
// existing debug!,info!, etc. macros and still get the log data
// to the receiver thread.
static LOGGER: OnceCell<ChannelLogger> = OnceCell::new();

// these two tokens are used to construct a specially-constructed LogRec
// that signals that the log receiver thread should exit. These are only
// used when the process is exiting, and the log handler checks for and
// excludes these tokens to prevent shutting down early.
const CLOSE_TOKEN: &str = "<<<close>>>";
const CLOSE_NUM: u32 = u32::MAX;

/// A `Send`able version of log::Record. The channel logger
/// converts Record into this before sending. The receiver
/// logs data from LogRec
#[derive(Clone)]
pub struct LogRec {
    level: Level,
    target: String,
    args: String,
    #[allow(dead_code)]
    module_path: Option<&'static str>,
    file: Option<&'static str>,
    line: Option<u32>,
}

impl<'a> From<&'a log::Record<'a>> for LogRec {
    fn from(rec: &'a log::Record) -> LogRec {
        LogRec {
            level: rec.level(),
            target: rec.target().to_string(),
            args: rec.args().to_string(),
            module_path: rec.module_path_static(),
            file: rec.file_static(),
            line: rec.line(),
        }
    }
}

impl log::Log for ChannelLogger {
    /// returns true if logging is enabled.
    /// This can be queried before calling log (or any of the macros), if
    /// generation of the log message may be "expensive"
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
        //LOGGER.get().is_some() && log::log_enabled!(target: metadata.target(), metadata.level())
    }

    /// sends a log record into the channel
    fn log(&self, record: &Record) {
        // avoid converting and sending logs if they are disabled
        // log::Level is ordered : error=1, trace=5
        if record.level() <= log::max_level() {
            // don't log close signal accidentally
            if matches!(record.line(), Some(CLOSE_NUM))
                && matches!(record.file(), Some(CLOSE_TOKEN))
            {
                // not allowed - this is our close signal
                return;
            }
            if self.tx.send(record.into()).is_err() {
                // log channel closed, send to stderr instead
                eprintln!(
                    "{}:{} -- {}",
                    record.level(),
                    record.target(),
                    record.args()
                );
            }
        }
    }

    fn flush(&self) {}
}

/// Sets the system logger to log all messages (debug!, info!, etc.)
/// through a crossbeam channel. The caller can pass the receiver to
/// init_receiver() to log to a file, or implement another receiver to
/// perform other processing (such as sending to syslog, or a remote logger).
/// An error return means another logger was already initialized,
/// and rx should be dropped.
pub fn init_logger() -> Result<Receiver, String> {
    let (tx, rx) = crossbeam::channel::bounded::<LogRec>(MESSAGE_LIMIT);
    LOGGER
        .set(ChannelLogger { tx })
        .map_err(|_| "log instance init failure".to_string())?;
    if let Err(e) = log::set_logger(
        LOGGER
            .get()
            .ok_or_else(|| "logger init failure".to_string())?,
    ) {
        Err(format!("logger init error: {}", e))
    } else {
        use std::str::FromStr as _;
        let mut detail_level = log::LevelFilter::Info;
        if let Ok(level) = std::env::var("RUST_LOG") {
            let level = if let Some((left, _)) = level.split_once(',') {
                left
            } else {
                &level
            };
            if let Ok(level) = log::LevelFilter::from_str(level) {
                detail_level = level;
            }
        }
        log::set_max_level(detail_level);
        Ok(rx)
    }
}

pub fn stop_receiver() {
    // log something that should never occur in real scenario:
    // level == Error,  line number u32:max and file name "<<<close>>>", an illegal file name
    // and args is empty
    let _ = LOGGER.get().unwrap().tx.send({
        LogRec {
            level: Level::Error,
            args: String::default(),
            target: String::default(),
            module_path: None,
            file: Some(CLOSE_TOKEN),
            line: Some(CLOSE_NUM),
        }
    });
}

/// Create background thread that uses the provided logger to log
/// all records sent by the sender. If the user wants to change the
/// log format, or log to a different location (file, syslog, or other
/// service), they should not call init_receiver, but instead create
/// their own function to handle LogRec events from the channel.
pub fn init_receiver(log_rx: Receiver) {
    use std::io::Write;

    // this works with either a std::thread or a tokio::task::spawn_blocking
    let _join = std::thread::spawn(move || {
        while let Ok(log_rec) = log_rx.recv() {
            // check for shutdown signal
            if matches!(log_rec.line, Some(CLOSE_NUM))
                && matches!(log_rec.file.as_ref(), Some(&CLOSE_TOKEN))
            {
                break;
            }
            let s = log_rec.to_string();
            let mut stderr = std::io::stderr();
            let _ = stderr.write_all(s.as_bytes());

            // It would have been preferable to generate a log::Record,
            // then we could call a "base_logger.log(record)", and let
            // the user of this module use a logger of their choosing.
            // I wasn't able to get this constructor to work with
            // format_args!, so consequently, this module needs to
            // its own string formatting. Any user of this module's api
            // that wants to modify the output would need to capture
            // the rx and do their own handling.
            /*
            let rec = Record::builder()
                .level(log_rec.level)
                .target(&log_rec.target)
                .module_path(log_rec.module_path.clone())
                .file(log_rec.file.clone())
                .line(log_rec.line.clone())
                .args(format_args!("{}", &log_rec.args))
                .build();
            base_logger.log(&rec);
             */
        }
    });
}

// an internally-used value to help vertically align consecutive log messsages.
static MAX_MODULE_WIDTH: AtomicUsize = AtomicUsize::new(0);

// All log lines have a newline appended.
// when running the provider stand-alone on linux, '\n' is sufficient,
// but when running under elixir, on linux, '\r\n' was needed for the
// error formatting to look right. Haven't tried other platforms yet,
// but the difference between linux-standalone and linux-in-elixir
// means querying the os for line endings won't always give the correct answer.
const NEWLINE: &str = "\r\n";

fn max_target_width(target: &str) -> usize {
    let max_width = MAX_MODULE_WIDTH.load(Ordering::Relaxed);
    if max_width < target.len() {
        MAX_MODULE_WIDTH.store(target.len(), Ordering::Relaxed);
        target.len()
    } else {
        max_width
    }
}

impl fmt::Debug for LogRec {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.to_string())
    }
}

impl ToString for LogRec {
    /// generates string representation of log including line terminator
    fn to_string(&self) -> String {
        let target = &self.target;
        let max_width = max_target_width(target);
        let target = Padded {
            value: target,
            width: max_width,
        };
        format!("{} {} > {}{}", self.level, target, &self.args, NEWLINE)
    }
}

struct Padded<T> {
    value: T,
    width: usize,
}

impl<T: fmt::Display> fmt::Display for Padded<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{: <width$}", self.value, width = self.width)
    }
}
