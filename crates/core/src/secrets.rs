use std::fmt::{Debug, Formatter, Result};

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone)]
// This tagging allows deserializers to know whether the secret is a string or bytes.
// This is especially necessary for languages where strings and bytes are treated very similarly.
#[serde(tag = "kind", content = "value")]
pub enum SecretValue {
    String(String),
    Bytes(Vec<u8>),
}

impl SecretValue {
    /// Utility function for retrieving a string slice from this [`SecretValue`], if possible.
    ///
    /// If the secret does not contain a string, `None` is returned
    #[must_use]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            SecretValue::String(s) => Some(s),
            SecretValue::Bytes(_) => None,
        }
    }

    /// Utility function for retrieving bytes from this [`SecretValue`], if possible.
    ///
    /// If the secret does not contain bytes, `None` is returned
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            SecretValue::String(_) => None,
            SecretValue::Bytes(b) => Some(b),
        }
    }
}

/// Debug implementation that doesn't log the secret value
impl Debug for SecretValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            SecretValue::String(_) => write!(f, "string(redacted)"),
            SecretValue::Bytes(_) => write!(f, "bytes(redacted)"),
        }
    }
}
