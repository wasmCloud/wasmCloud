//! implementations of source code formatters (rustfmt, gofmt)
//!

use crate::{
    error::{Error, Result},
    gen::SourceFormatter,
};

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
            edition: "2021".to_string(),
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
        let mut args = vec![
            "--edition",
            &self.edition,
            "--config",
            "format_generated_files=true",
        ];
        args.extend(self.extra.iter().map(|s| s.as_str()));
        args.extend(source_files.iter());
        run_command(&self.program, &args)?;
        Ok(())
    }
}

pub struct GoSourceFormatter {
    program: String,
    args: Vec<String>,
}

impl Default for GoSourceFormatter {
    fn default() -> Self {
        GoSourceFormatter {
            program: "go".into(),
            args: vec!["fmt".into()],
        }
    }
}

impl SourceFormatter for GoSourceFormatter {
    fn run(&self, source_files: &[&str]) -> Result<()> {
        // we get an error if the files are in different packages,
        // so run once per file in case output packages differ
        for f in source_files {
            // TODO(future): caller converts array of paths to array of str, and we convert back to path again.
            // ... we could change the api to this fn to take array of Path or PathBuf instead
            let mut args = self.args.clone();
            let path = std::fs::canonicalize(f)?;
            args.push(path.to_string_lossy().to_string());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            if let Err(e) = run_command(&self.program, &str_args) {
                eprintln!("Warning:  formatting '{}': {}", path.display(), e);
            }
        }
        Ok(())
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
