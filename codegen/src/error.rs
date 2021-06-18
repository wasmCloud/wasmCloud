//use crate::strings::{to_pascal_case, to_snake_case};
use std::string::ToString;
use thiserror::Error as ThisError;

pub type Result<T> = std::result::Result<T, Error>;

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

    #[error("BigInteger is currently an unsupported type")]
    UnsupportedBigInteger,
    #[error("BigDecimal is currently an unsupported type")]
    UnsupportedBigDecimal,
    #[error("Timestamp is currently an unsupported type")]
    UnsupportedTimestamp,
    #[error("Document is currently an unsupported type")]
    UnsupportedDocument,
    #[error("{0} is an unsupported type")]
    UnsupportedType(String),

    #[error("handlebars error: {0}")]
    Handlebars(String),

    #[error("visitor: {0}")]
    Inner(String),
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
