use serde::{Deserialize, Serialize};

/// Log entry
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogEntry {
    /// severity level: debug,info,warn,error
    #[serde(default)]
    pub level: String,
    /// message to log
    #[serde(default)]
    pub text: String,
}
