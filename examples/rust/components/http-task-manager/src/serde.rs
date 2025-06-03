//! This lib contains patches to make objects serializable via serde
//!  
//! This is necessary because of the following bug in upstream wit-bindgen:
//! https://github.com/bytecodealliance/wit-bindgen/issues/812

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::bindings::wasmcloud::postgres::types::{Date, Offset, Time, Timestamp};
use crate::bindings::wasmcloud::task_manager::types::TimestampTz;
use crate::bindings::wasmcloud::task_manager::types::{Task, TaskStatus};

#[derive(Serialize, Deserialize)]
#[serde(remote = "TaskStatus")]
enum TaskStatusDeserProxy {
    Pending,
    Leased,
    Completed,
    Failed,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Offset")]
enum OffsetDeserProxy {
    EasternHemisphereSecs(i32),
    WesternHemisphereSecs(i32),
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "TimestampTz")]
struct TimestampTzDeserProxy {
    #[serde(with = "TimestampDeserProxy")]
    timestamp: Timestamp,
    #[serde(with = "OffsetDeserProxy")]
    offset: Offset,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Date")]
enum DateDeserProxy {
    PositiveInfinity,
    NegativeInfinity,
    Ymd((i32, u32, u32)),
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Time")]
struct TimeDeserProxy {
    hour: u32,
    min: u32,
    sec: u32,
    micro: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Timestamp")]
struct TimestampDeserProxy {
    #[serde(with = "DateDeserProxy")]
    date: Date,
    #[serde(with = "TimeDeserProxy")]
    time: Time,
}

/// Unfortunately, since serde for remote types cannot handle Option<T>
/// or similar wrappers, we must write a custom Deserialize implementation
///
/// see: https://serde.rs/remote-derive.html
/// see: https://github.com/serde-rs/serde/issues/1301
impl<'de> Deserialize<'de> for TimestampTz {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper(#[serde(with = "TimestampTzDeserProxy")] TimestampTz);
        Helper::deserialize(deserializer).map(|v| v.0)
    }
}

/// Unfortunately, since serde for remote types cannot handle Option<T>
/// or similar wrappers, we must write a custom Serialize implementation
///
/// see: https://serde.rs/remote-derive.html
/// see: https://github.com/serde-rs/serde/issues/1301
impl Serialize for TimestampTz {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a>(#[serde(with = "TimestampTzDeserProxy")] &'a TimestampTz);
        Helper(self).serialize(serializer)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Task")]
#[allow(unused)]
struct TaskDeserProxy {
    id: String,
    #[serde(with = "TaskStatusDeserProxy")]
    status: TaskStatus,
    group_id: String,
    data_json: Option<String>,
    last_failed_at: Option<TimestampTz>,
    last_failure_reason: Option<String>,
    leased_at: Option<TimestampTz>,
    lease_worker_id: Option<String>,
    completed_at: Option<TimestampTz>,
    submitted_at: TimestampTz,
    last_updated_at: TimestampTz,
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper(#[serde(with = "TaskDeserProxy")] Task);
        Helper::deserialize(deserializer).map(|v| v.0)
    }
}

impl Serialize for Task {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a>(#[serde(with = "TaskDeserProxy")] &'a Task);
        Helper(self).serialize(serializer)
    }
}
