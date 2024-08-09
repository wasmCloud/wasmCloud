use std::str::FromStr as _;

use anyhow::{bail, Context as _, Result};
use chrono::{DateTime, FixedOffset, NaiveDate};

use crate::bindings::wasmcloud::postgres::types::{
    Date, Offset, PgValue, ResultRow, ResultRowEntry, Time, Timestamp, TimestampTz,
};
use crate::bindings::wasmcloud::task_manager::types::TaskStatus;
use crate::Task;

pub(crate) mod migrations;
pub(crate) mod queries;

/// Convert an offset to a chrono aware offset
impl Offset {
    fn chrono_offset(&self) -> Result<FixedOffset> {
        match self {
            Offset::EasternHemisphereSecs(secs) => {
                FixedOffset::east_opt(*secs).context("failed to convert eastern offset")
            }
            Offset::WesternHemisphereSecs(secs) => {
                FixedOffset::west_opt(*secs).context("failed to convert western offset")
            }
        }
    }
}

impl TryFrom<&TimestampTz> for DateTime<FixedOffset> {
    type Error = anyhow::Error;

    fn try_from(ttz: &TimestampTz) -> Result<DateTime<FixedOffset>> {
        if let TimestampTz {
            timestamp:
                Timestamp {
                    date: Date::Ymd((y, m, d)),
                    time:
                        Time {
                            hour,
                            min,
                            sec,
                            micro,
                        },
                },
            offset,
        } = ttz
        {
            return Ok(DateTime::from_naive_utc_and_offset(
                NaiveDate::from_ymd_opt(*y, *m, *d)
                    .context("failed to build date")?
                    .and_hms_micro_opt(*hour, *min, *sec, *micro)
                    .context("failed to add hours/min/seconds")?,
                offset.chrono_offset()?,
            ));
        }
        bail!("invalid timestamp tz object");
    }
}

/// Retrieve a value by named column which is expected to be a string
pub(crate) fn extract_nonnull_string_column<'a>(
    row: &'a ResultRow,
    column_name: &str,
) -> Result<&'a String> {
    match row.iter().find(|entry| entry.column_name == column_name) {
        Some(ResultRowEntry {
            value: PgValue::Text(s),
            ..
        }) => Ok(s),
        _ => bail!("required column [{column_name}] is missing/null"),
    }
}

/// Retrieve a value by named column which is expected to be a Postgres UUID
pub(crate) fn extract_nonnull_uuid_column<'a>(
    row: &'a ResultRow,
    column_name: &str,
) -> Result<&'a String> {
    match row.iter().find(|entry| entry.column_name == column_name) {
        Some(ResultRowEntry {
            value: PgValue::Uuid(s),
            ..
        }) => Ok(s),
        _ => bail!("required column [{column_name}] is missing/null"),
    }
}

/// Retrieve a value by named column which is expected to be a Postgres JSON
pub(crate) fn extract_nonnull_json_column<'a>(
    row: &'a ResultRow,
    column_name: &str,
) -> Result<&'a String> {
    match row.iter().find(|entry| entry.column_name == column_name) {
        Some(ResultRowEntry {
            value: PgValue::Json(s) | PgValue::Jsonb(s),
            ..
        }) => Ok(s),
        _ => bail!("required column [{column_name}] is missing/null"),
    }
}

/// Retrieve a value by named column which is expected to be a timestamptz
#[allow(unused)]
fn extract_timestamptz_column(row: &ResultRow, column_name: &str) -> Result<DateTime<FixedOffset>> {
    match row.iter().find(|entry| entry.column_name == column_name) {
        Some(ResultRowEntry {
            value: PgValue::TimestampTz(timestamp),
            ..
        }) => timestamp.try_into(),
        _ => bail!("required timestamptz column [{column_name}] is missing/null"),
    }
}

/// Retrieve a PgValue by named column which is expected to be a timestamptz
fn extract_timestamptz_column_pgvalue(row: &ResultRow, column_name: &str) -> Result<TimestampTz> {
    match row.iter().find(|entry| entry.column_name == column_name) {
        Some(ResultRowEntry {
            value:
                PgValue::TimestampTz(TimestampTz {
                    timestamp: Timestamp { date, time },
                    offset,
                }),
            ..
        }) => Ok(TimestampTz {
            timestamp: Timestamp {
                date: *date,
                time: *time,
            },
            offset: *offset,
        }),
        _ => bail!("required timestamptz column [{column_name}] is missing/null"),
    }
}

impl TryFrom<&ResultRow> for Task {
    type Error = anyhow::Error;

    fn try_from(row: &ResultRow) -> Result<Task> {
        let id = extract_nonnull_uuid_column(row, "id")
            .context("failed to find id column for Task")?
            .to_string();
        let group_id = extract_nonnull_string_column(row, "group_id")
            .context("failed to find group_id column for Task")?
            .to_string();
        let status = extract_nonnull_string_column(row, "status")
            .context("failed to find status column for Task")?;
        let data_json = Some(
            extract_nonnull_json_column(row, "data_json")
                .map(ToString::to_string)
                .context("failed to find data_json column for Task")?,
        );
        let last_failed_at = extract_timestamptz_column_pgvalue(row, "last_failed_at").ok();
        let last_failure_reason = extract_nonnull_string_column(row, "last_failure_reason")
            .ok()
            .cloned();
        let lease_worker_id = extract_nonnull_string_column(row, "lease_worker_id")
            .ok()
            .cloned();
        let leased_at = extract_timestamptz_column_pgvalue(row, "leased_at").ok();
        let completed_at = extract_timestamptz_column_pgvalue(row, "completed_at").ok();
        let submitted_at = extract_timestamptz_column_pgvalue(row, "submitted_at")
            .context("failed to find submitted_at column for Task")?;
        let last_updated_at = extract_timestamptz_column_pgvalue(row, "last_updated_at")
            .context("failed to find last_updated_at column for Task")?;
        Ok(Task {
            id,
            status: TaskStatus::from_str(status).with_context(|| {
                format!("Invalid status [{status}], failed to convert to TaskStatus")
            })?,
            group_id,
            data_json,
            last_failed_at,
            last_failure_reason,
            leased_at,
            lease_worker_id,
            completed_at,
            submitted_at,
            last_updated_at,
        })
    }
}
