use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProjectPaths {
    project_dir: PathBuf,
}

impl ProjectPaths {
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
        }
    }

    pub fn from_current_dir() -> Result<Self> {
        Ok(Self::new(
            std::env::current_dir().context("failed to get current directory")?,
        ))
    }

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    pub fn with_project_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.project_dir = dir.into();
        self
    }
}

// Default implementation uses current directory
impl Default for ProjectPaths {
    fn default() -> Self {
        Self::from_current_dir().unwrap_or_else(|_| Self::new("."))
    }
}
