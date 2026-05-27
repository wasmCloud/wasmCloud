//! Keyvalue helper for the request counter.

use anyhow::{Context, Result, anyhow};

use crate::bindings::wasi::keyvalue::{atomics::increment, store::open};

pub(crate) const COUNTER_KEY: &str = "request-count";

pub(crate) fn increment_counter() -> Result<u64> {
    let bucket = open("").map_err(|e| anyhow!("failed to open keyvalue bucket: {e:?}"))?;
    increment(&bucket, COUNTER_KEY, 1).context("failed to increment counter")
}
