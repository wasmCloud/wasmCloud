use std::{collections::BTreeMap, fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use toml::Value as TomlValue;

use crate::error::Error;

/// Output languages for code generation
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputLanguage {
    /// used for code generation of language-independent project files
    Poly,
    /// HTML documentation
    Html,
    /// Rust
    Rust,
    /// AssemblyScript (not currently supported)
    AssemblyScript,
    /// TinyGo (not currently supported)
    TinyGo,
    /// Go (not currently supported)
    Go,
    /// Python
    Python,
    /// C++ (not currently supported)
    Clang,
}

impl std::str::FromStr for OutputLanguage {
    type Err = Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "poly" => Ok(OutputLanguage::Poly),
            "html" => Ok(OutputLanguage::Html),
            "rust" => Ok(OutputLanguage::Rust),
            "assemblyscript" => Ok(OutputLanguage::AssemblyScript),
            "tinygo" => Ok(OutputLanguage::TinyGo),
            "go" => Ok(OutputLanguage::Go),
            "python" => Ok(OutputLanguage::Python),
            "c++" | "clang" => Ok(OutputLanguage::Clang),
            _ => Err(Error::UnsupportedLanguage(s.to_string())),
        }
    }
}

impl fmt::Display for OutputLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                OutputLanguage::Poly => "Poly",
                OutputLanguage::Html => "Html",
                OutputLanguage::Rust => "Rust",
                OutputLanguage::AssemblyScript => "AssemblyScript",
                OutputLanguage::TinyGo => "TinyGo",
                OutputLanguage::Go => "Go",
                OutputLanguage::Python => "Python",
                OutputLanguage::Clang => "Clang",
            }
        )
    }
}

impl OutputLanguage {
    // returns the primary extension for the langauge
    pub fn extension(&self) -> &'static str {
        match self {
            OutputLanguage::Rust => "rs",
            OutputLanguage::TinyGo | OutputLanguage::Go => "go",
            OutputLanguage::AssemblyScript => "rs",
            OutputLanguage::Python => "rs",
            OutputLanguage::Poly => "",
            OutputLanguage::Html => "html",
            OutputLanguage::Clang => "c",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CodegenConfig {
    /// model inputs
    #[serde(default)]
    pub models: Vec<ModelSource>,

    /// This can be set by the cli to limit the languages generated.
    /// Without this flag, all languages in the toml file are generated.
    #[serde(default)]
    pub output_languages: Vec<OutputLanguage>,

    /// Language-specific output configuration
    #[serde(flatten)]
    pub languages: BTreeMap<OutputLanguage, LanguageConfig>,

    /// The directory containing the codegen.toml file, and the base_dir
    /// used for evaluating all relative paths in the file.
    /// This is not set inside the toml file but is set by the file reader,
    /// It is always set to an absolute path
    #[serde(default)]
    pub base_dir: PathBuf,
}

/// Source directory or url prefix for finding model files
/// For Paths, the `path` and `files` can be model files, or directories, which will
/// be searched recursively for model files with `.json` or `.smithy` extensions.
/// `files` array is optional if url or path directly references a model file,
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ModelSource {
    Url {
        url: String,
        #[serde(default)]
        files: Vec<String>,
    },
    Path {
        path: PathBuf,
        #[serde(default)]
        files: Vec<String>,
    },
}

pub enum ModelSourceKind {
    Url(String),
    Path(PathBuf),
}

impl FromStr for ModelSource {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(if s.starts_with("https:") || s.starts_with("http:") {
            ModelSource::Url {
                url: s.to_string(),
                files: Vec::default(),
            }
        } else {
            ModelSource::Path { path: s.into(), files: Vec::default() }
        })
    }
}

impl ModelSource {
    /// convenience function to create a ModelSource for a single file path
    pub fn from_file<P: Into<std::path::PathBuf>>(path: P) -> ModelSource {
        ModelSource::Path { path: path.into(), files: Vec::default() }
    }
}

impl fmt::Display for ModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ModelSource::Url { url, files: _ } => format!("url({})", url),
                ModelSource::Path { path, files: _ } => format!("path({})", path.display()),
            }
        )
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LanguageConfig {
    /// list of template files or template folders for importing templates.
    /// Overwrites any compiled-in templates with the same name(s)
    #[serde(default)]
    pub templates: Option<PathBuf>,

    /// Output directory. Required.
    /// (with weld cli, this will be relative to the output-dir on the command line)
    pub output_dir: PathBuf,

    /// Additional parameters
    #[serde(default)]
    pub parameters: BTreeMap<String, TomlValue>,

    /// source-code formatter
    /// first item in vec should be program, rest are args
    /// For languages other than rust, the formatter _only_ runs if defined in codegen.toml
    /// example: [ "goimports", "-w" ]
    /// example: [ "rustfmt", "--edition", "2021" ]
    #[serde(default)]
    pub formatter: Vec<String>,

    /// Settings specific to individual output files
    #[serde(default)]
    pub files: Vec<OutputFile>,
}

/// Output-file specific settings
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct OutputFile {
    /// path to output file, relative to language output_dir. Required.
    pub path: PathBuf,

    /// name of handlebars template file (without hbs extension)
    /// Not used for files generated by codegen
    #[serde(default)]
    pub hbs: Option<String>,

    /// True if file should be generated only for 'create' operations
    #[serde(default)]
    pub create_only: bool,

    /// Optionally, limit code generation for this file to shapes in this namespace
    #[serde(default)]
    pub namespace: Option<String>,

    /// optional additional parameters for this file
    #[serde(flatten)]
    pub params: BTreeMap<String, TomlValue>,
}

impl FromStr for CodegenConfig {
    type Err = Error;

    fn from_str(content: &str) -> std::result::Result<CodegenConfig, Self::Err> {
        let config =
            toml::from_str(content).map_err(|e| Error::Other(format!("codegen: {}", e)))?;
        Ok(config)
    }
}
