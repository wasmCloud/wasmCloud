//! implementations of source code formatters (rustfmt, gofmt)
//!

use crate::{gen::SourceFormatter, Error, Result};

/// A formatter that does not format any code
#[derive(Default)]
pub struct NullFormatter {}
impl SourceFormatter for NullFormatter {}

/// Format rust source using rustfmt
pub struct RustSourceFormatter {
    /// either 'rustfmt', (the default, assumes ~/.cargo/bin is in your path,
    /// or a path to an executable
    program: String,
    /// rust Edition, default "2018"
    edition: String,
    /// any additional args
    extra: Vec<String>,
}

impl Default for RustSourceFormatter {
    fn default() -> Self {
        RustSourceFormatter {
            program: "rustfmt".to_string(),
            edition: "2018".to_string(),
            extra: Vec::new(),
        }
    }
}

impl SourceFormatter for RustSourceFormatter {
    fn run(&self, source_files: &[&str]) -> Result<()> {
        if !matches!(self.edition.as_str(), "2015" | "2018" | "2021") {
            return Err(Error::Formatter(format!(
                "invalid edition: {}",
                self.edition
            )));
        }
        let mut args = vec!["--edition", &self.edition];
        args.extend(self.extra.iter().map(|s| s.as_str()));
        args.extend(source_files.iter());
        run_command(&self.program, &args)?;
        Ok(())
    }

    fn include(&self, path: &std::path::Path) -> bool {
        crate::codegen_rust::is_rust_source(path)
    }
}

/// execute the program with args
pub(crate) fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let mut child = std::process::Command::new(program)
        .args(args.iter())
        .spawn()
        .map_err(|e| Error::Formatter(format!("failed to start: {}", e)))?;

    let code = child
        .wait()
        .map_err(|e| Error::Formatter(format!("failed waiting for formatter: {}", e)))?;
    if !code.success() {
        return Err(Error::Formatter(code.to_string()));
    }
    Ok(())
}
