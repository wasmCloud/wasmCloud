//use crate::strings::{to_pascal_case, to_snake_case};
use std::string::ToString;

use thiserror::Error as ThisError;

pub type Result<T> = std::result::Result<T, Error>;

#[non_exhaustive]
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("missing input file: {0}")]
    MissingFile(String),

    #[error("unsupported output language : {0}")]
    UnsupportedLanguage(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("smithy error: {0}")]
    Model(String),

    #[error("identifier {0} has an unsupported model shape: {1}")]
    UnsupportedShape(String, String),

    #[error("service {0} contains unknown operation: {1}")]
    OperationNotFound(String, String),

    #[error("invalid model: {0}")]
    InvalidModel(String),

    #[error("{0} is an unsupported type")]
    UnsupportedType(String),

    #[error("handlebars error: {0}")]
    Handlebars(String),

    #[error("ser-deser : {0}")]
    Serde(String),

    #[error("source formatter {0}")]
    Formatter(String),

    // build error
    #[error("{0}")]
    Build(String),

    // catch-all - use descriptive error text
    #[error("{0}")]
    Other(String),

    #[error("BigInteger is currently an unsupported type")]
    UnsupportedBigInteger,

    #[error("BigDecimal is currently an unsupported type")]
    UnsupportedBigDecimal,
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e.to_string())
    }
}

impl From<handlebars::TemplateError> for Error {
    fn from(e: handlebars::TemplateError) -> Error {
        Error::Handlebars(e.to_string())
    }
}

impl From<handlebars::RenderError> for Error {
    fn from(e: handlebars::RenderError) -> Error {
        Error::Handlebars(e.to_string())
    }
}

impl From<atelier_core::error::Error> for Error {
    fn from(e: atelier_core::error::Error) -> Error {
        Error::Model(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Serde(e.to_string())
    }
}

/// Print a warning message to the user.
/// If we are running within build.rs, stdout and stderr can't be used because
/// they are redirected to a build output folder, so use the cargo:warning stdout hook
pub(crate) fn print_warning(msg: &str) {
    if std::env::var("OUT_DIR").is_ok() && std::env::var("CARGO").is_ok() {
        println!("cargo:warning= {msg}");
    } else {
        eprintln!("Warning: {msg}");
    }
}
