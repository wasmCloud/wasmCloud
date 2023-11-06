// Copyright 2015-2018 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{error::Error as StdError, fmt};

use wasmparser::BinaryReaderError;

/// An error that can contain wascap-specific context
#[derive(Debug)]
pub struct Error(Box<ErrorKind>);

pub(crate) fn new(kind: ErrorKind) -> Error {
    Error(Box::new(kind))
}

#[derive(Debug)]
pub enum ErrorKind {
    Serialize(serde_json::error::Error),
    Encryption(nkeys::error::Error),
    Decode(data_encoding::DecodeError),
    UTF8(std::string::FromUtf8Error),
    Token(String),
    InvalidCapability,
    WasmElement(String),
    IO(std::io::Error),
    InvalidModuleHash,
    ExpiredToken,
    TokenTooEarly,
    InvalidAlgorithm,
    MissingIssuer,
    MissingSubject,
}

impl Error {
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.0
    }

    #[must_use]
    pub fn into_kind(self) -> ErrorKind {
        *self.0
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self.0 {
            ErrorKind::Serialize(_) => "Serialization failure",
            ErrorKind::Encryption(_) => "Encryption failure",
            ErrorKind::Decode(_) => "Decode failure",
            ErrorKind::UTF8(_) => "UTF8 failure",
            ErrorKind::Token(_) => "JWT failure",
            ErrorKind::InvalidCapability => "Invalid Capability",
            ErrorKind::WasmElement(_) => "WebAssembly element",
            ErrorKind::IO(_) => "I/O error",
            ErrorKind::InvalidModuleHash => "Invalid Module Hash",
            ErrorKind::ExpiredToken => "Token has expired",
            ErrorKind::TokenTooEarly => "Token cannot be used yet",
            ErrorKind::InvalidAlgorithm => "Invalid JWT algorithm",
            ErrorKind::MissingIssuer => "Missing issuer claim",
            ErrorKind::MissingSubject => "Missing sub claim",
        }
    }

    fn cause(&self) -> Option<&dyn StdError> {
        match *self.0 {
            ErrorKind::Serialize(ref err) => Some(err),
            ErrorKind::Encryption(ref err) => Some(err),
            ErrorKind::Decode(ref err) => Some(err),
            ErrorKind::UTF8(ref err) => Some(err),
            ErrorKind::IO(ref err) => Some(err),
            ErrorKind::Token(_)
            | ErrorKind::InvalidCapability
            | ErrorKind::WasmElement(_)
            | ErrorKind::InvalidModuleHash
            | ErrorKind::ExpiredToken
            | ErrorKind::TokenTooEarly
            | ErrorKind::InvalidAlgorithm
            | ErrorKind::MissingIssuer
            | ErrorKind::MissingSubject => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self.0 {
            ErrorKind::Serialize(ref err) => write!(f, "Serialization error: {err}"),
            ErrorKind::Encryption(ref err) => write!(f, "Encryption error: {err}"),
            ErrorKind::Decode(ref err) => write!(f, "Decode error: {err}"),
            ErrorKind::UTF8(ref err) => write!(f, "UTF8 error: {err}"),
            ErrorKind::Token(ref err) => write!(f, "JWT error: {err}"),
            ErrorKind::InvalidCapability => write!(f, "Invalid capability"),
            ErrorKind::WasmElement(ref err) => write!(f, "Wasm Element error: {err}"),
            ErrorKind::IO(ref err) => write!(f, "I/O error: {err}"),
            ErrorKind::InvalidModuleHash => write!(f, "Invalid module hash"),
            ErrorKind::ExpiredToken => write!(f, "Module token has expired"),
            ErrorKind::TokenTooEarly => write!(f, "Module cannot be used yet"),
            ErrorKind::InvalidAlgorithm => {
                write!(f, "Invalid JWT algorithm. WASCAP only supports Ed25519")
            }
            ErrorKind::MissingIssuer => {
                write!(
                    f,
                    "Invalid JWT. WASCAP requires an issuer claim to be present"
                )
            }
            ErrorKind::MissingSubject => {
                write!(f, "Invalid JWT. WASCAP requires a sub claim to be present")
            }
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Error {
        Error(Box::new(ErrorKind::IO(source)))
    }
}

impl From<BinaryReaderError> for Error {
    fn from(source: BinaryReaderError) -> Error {
        let io_error = ::std::io::Error::new(::std::io::ErrorKind::Other, source.to_string());
        Error(Box::new(ErrorKind::IO(io_error)))
    }
}

impl From<serde_json::error::Error> for Error {
    fn from(source: serde_json::error::Error) -> Error {
        Error(Box::new(ErrorKind::Serialize(source)))
    }
}

impl From<data_encoding::DecodeError> for Error {
    fn from(source: data_encoding::DecodeError) -> Error {
        Error(Box::new(ErrorKind::Decode(source)))
    }
}

impl From<nkeys::error::Error> for Error {
    fn from(source: nkeys::error::Error) -> Error {
        Error(Box::new(ErrorKind::Encryption(source)))
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(source: std::string::FromUtf8Error) -> Error {
        Error(Box::new(ErrorKind::UTF8(source)))
    }
}
