//! Common config constants and functions for loading, finding, and consuming configuration data
use std::{
    fs,
    io::{Error, ErrorKind, Result as IoResult},
    path::PathBuf,
};

const WASH_DIR: &str = ".wash";

pub const DOWNLOADS_DIR: &str = "downloads";
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: &str = "4222";
pub const DEFAULT_LATTICE_PREFIX: &str = "default";
pub const DEFAULT_NATS_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_START_ACTOR_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_START_PROVIDER_TIMEOUT_MS: u64 = 60_000;

/// Get the path to the `.wash` configuration directory. Creates the directory if it does not exist.
pub fn cfg_dir() -> IoResult<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "No home directory found. Please set $HOME.",
        )
    })?;

    let wash = home.join(WASH_DIR);

    if !wash.exists() {
        fs::create_dir_all(&wash)?;
    }

    Ok(wash)
}

pub fn model_cache_dir() -> IoResult<PathBuf> {
    weld_codegen::weld_cache_dir().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}

pub fn downloads_dir() -> IoResult<PathBuf> {
    cfg_dir().map(|p| p.join(DOWNLOADS_DIR))
}
