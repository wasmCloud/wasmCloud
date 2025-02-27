use std::borrow::Cow;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use crate::lib::cli::OutputKind;

// For more spinners check out the cli-spinners project:
// https://github.com/sindresorhus/cli-spinners/blob/master/spinners.json
pub const DOTS_12: &[&str; 56] = &[
    "⢀⠀", "⡀⠀", "⠄⠀", "⢂⠀", "⡂⠀", "⠅⠀", "⢃⠀", "⡃⠀", "⠍⠀", "⢋⠀", "⡋⠀", "⠍⠁", "⢋⠁", "⡋⠁", "⠍⠉", "⠋⠉",
    "⠋⠉", "⠉⠙", "⠉⠙", "⠉⠩", "⠈⢙", "⠈⡙", "⢈⠩", "⡀⢙", "⠄⡙", "⢂⠩", "⡂⢘", "⠅⡘", "⢃⠨", "⡃⢐", "⠍⡐", "⢋⠠",
    "⡋⢀", "⠍⡁", "⢋⠁", "⡋⠁", "⠍⠉", "⠋⠉", "⠋⠉", "⠉⠙", "⠉⠙", "⠉⠩", "⠈⢙", "⠈⡙", "⠈⠩", "⠀⢙", "⠀⡙", "⠀⠩",
    "⠀⢘", "⠀⡘", "⠀⠨", "⠀⢐", "⠀⡐", "⠀⠠", "⠀⢀", "⠀⡀",
];

pub struct Spinner {
    spinner: Option<ProgressBar>,
}

impl Spinner {
    pub fn new(output_kind: &OutputKind) -> Result<Self> {
        match output_kind {
            OutputKind::Text => {
                let style = ProgressStyle::default_spinner()
                    .tick_strings(DOTS_12)
                    .template("{prefix:.bold.dim} {spinner:.bold.dim} {wide_msg:.bold.dim}")?;

                let spinner = ProgressBar::new_spinner().with_style(style);

                spinner.enable_steady_tick(std::time::Duration::from_millis(200));
                Ok(Self {
                    spinner: Some(spinner),
                })
            }
            OutputKind::Json => Ok(Self { spinner: None }),
        }
    }

    /// Handles updating the spinner for text output
    /// JSON output will be corrupted with a spinner
    pub fn update_spinner_message(&self, msg: impl Into<Cow<'static, str>>) {
        if let Some(spinner) = &self.spinner {
            spinner.set_prefix(">>>");
            spinner.set_message(msg);
        }
    }

    pub fn finish_and_clear(&self) {
        if let Some(progress_bar) = &self.spinner {
            progress_bar.finish_and_clear();
        }
    }
}
