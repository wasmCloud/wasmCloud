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

/// Debug implementation that doesn't log the secret value
impl Debug for SecretValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            SecretValue::String(_) => write!(f, "string(redacted)"),
            SecretValue::Bytes(_) => write!(f, "bytes(redacted)"),
        }
    }
}
