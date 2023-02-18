//! implementations of source code formatters (rustfmt, gofmt)
//!

use crate::error::{Error, Result};

/// Formats source code
#[allow(unused_variables)]
pub trait SourceFormatter {
    /// run formatter on all files
    /// Default implementation does nothing
    fn run(&self, source_files: &[&str]) -> Result<()> {
        Ok(())
    }
}

/// A formatter that does not format any code
#[derive(Default)]
pub struct NullFormatter {}
impl SourceFormatter for NullFormatter {}

/// execute the program with args
pub(crate) fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let mut child = std::process::Command::new(program)
        .args(args.iter())
        .spawn()
        .map_err(|e| Error::Formatter(format!("failed to start: {e}")))?;

    let code = child
        .wait()
        .map_err(|e| Error::Formatter(format!("failed waiting for formatter: {e}")))?;
    if !code.success() {
        return Err(Error::Formatter(code.to_string()));
    }
    Ok(())
}
