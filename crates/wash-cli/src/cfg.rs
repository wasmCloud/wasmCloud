use anyhow::Result;
use std::{
    fs,
    io::{Error, ErrorKind},
    path::PathBuf,
};

const WASH_DIR: &str = ".wash";

/// Get the path to the `.wash` configuration directory.
/// Creates the directory if it does not exist.
pub fn cfg_dir() -> Result<PathBuf> {
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
