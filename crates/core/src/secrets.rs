use std::fmt::{Debug, Formatter, Result};

use serde::{Deserialize, Serialize};

use crate::bindings;

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

impl From<bindings::wrpc::extension::types::SecretValue> for SecretValue {
    fn from(value: bindings::wrpc::extension::types::SecretValue) -> Self {
        match value {
            bindings::wrpc::extension::types::SecretValue::String(s) => SecretValue::String(s),
            bindings::wrpc::extension::types::SecretValue::Bytes(b) => SecretValue::Bytes(b.into()),
        }
    }
}

impl From<SecretValue> for bindings::wrpc::extension::types::SecretValue {
    fn from(value: SecretValue) -> Self {
        match value {
            SecretValue::String(s) => bindings::wrpc::extension::types::SecretValue::String(s),
            SecretValue::Bytes(b) => bindings::wrpc::extension::types::SecretValue::Bytes(b.into()),
        }
    }
}

impl From<&bindings::wrpc::extension::types::SecretValue> for SecretValue {
    fn from(value: &bindings::wrpc::extension::types::SecretValue) -> Self {
        match value {
            bindings::wrpc::extension::types::SecretValue::String(s) => {
                SecretValue::String(s.clone())
            }
            bindings::wrpc::extension::types::SecretValue::Bytes(b) => {
                SecretValue::Bytes(b.to_vec())
            }
        }
    }
}

impl From<&SecretValue> for bindings::wrpc::extension::types::SecretValue {
    fn from(value: &SecretValue) -> Self {
        match value {
            SecretValue::String(s) => {
                bindings::wrpc::extension::types::SecretValue::String(s.clone())
            }
            SecretValue::Bytes(b) => {
                bindings::wrpc::extension::types::SecretValue::Bytes(b.clone().into())
            }
        }
    }
}

/// Convert wasmcloud_core::secrets::SecretValue to the host bindings SecretValue type
pub fn convert_secret_value(
    value: SecretValue,
) -> crate::bindings::wrpc::extension::types::SecretValue {
    match value {
        SecretValue::String(s) => crate::bindings::wrpc::extension::types::SecretValue::String(s),
        SecretValue::Bytes(b) => {
            crate::bindings::wrpc::extension::types::SecretValue::Bytes(b.into())
        }
    }
}
