//! Error-related utilities for use by `wash`

use std::error::Error;

use anyhow::anyhow;

/// Simple helper function to suggest running a host if no responders are found
pub(crate) fn suggest_run_host_error(
    e: Box<dyn Error + std::marker::Send + Sync>,
) -> anyhow::Error {
    if e.to_string().contains("no responders") {
        anyhow!("No responders found for config put request. Is a host running?")
    } else {
        anyhow!(e)
    }
}
