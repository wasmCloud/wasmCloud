//! Types and tools for basic validation of seeds and IDs used in configuration

use std::{convert::AsRef, fmt::Display, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// An error type describing the types of errors when parsing an ID
#[derive(Error, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// The key is the wrong type of ID or seed
    #[error(r#"found the prefix "{found}", but expected "{expected}""#)]
    InvalidKeyType { found: String, expected: String },
    /// The key does not have the proper length
    #[error("the key should be {expected} characters, but was {found} characters")]
    InvalidLength { found: usize, expected: usize },
}

/// A module (i.e. Actor) ID
pub type ModuleId = Id<'M'>;
/// A host ID
pub type ServerId = Id<'N'>;
/// A service (i.e. Provider) ID
pub type ServiceId = Id<'V'>;
/// A private key for a server
pub type ClusterSeed = Seed<'C'>;

/// A wrapper around specific ID types. This is not meant to be a full nkey, but simple validation
/// for use in serialized/deserialized types
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Id<const PREFIX: char>(String);

impl<const PREFIX: char> FromStr for Id<PREFIX> {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse(s, PREFIX, false)?))
    }
}

impl<const PREFIX: char> Display for Id<PREFIX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A wrapper around specific seed types. This is not meant to be a full nkey, but simple validation
/// for use in serialized/deserialized types
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Seed<const PREFIX: char>(String);

impl<const PREFIX: char> Display for Seed<PREFIX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // NOTE: We may want to make this not print the key in the future (maybe by only
        // implementing ToString rather than display)
        self.0.fmt(f)
    }
}

impl<const PREFIX: char> Default for Seed<PREFIX> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<const PREFIX: char> AsRef<str> for Seed<PREFIX> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<const PREFIX: char> FromStr for Seed<PREFIX> {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse(s, PREFIX, true)?))
    }
}

fn parse(value: &str, prefix: char, is_seed: bool) -> Result<String, ParseError> {
    let (len, prefix) = if is_seed {
        (58, format!("S{prefix}"))
    } else {
        (56, prefix.to_string())
    };

    let count = value.chars().count();
    if count != len {
        return Err(ParseError::InvalidLength {
            found: count,
            expected: len,
        });
    }

    if value.starts_with(&prefix) {
        Ok(value.to_string())
    } else {
        Err(ParseError::InvalidKeyType {
            found: value.chars().take(prefix.chars().count()).collect(),
            expected: prefix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(
		"SC00000000000000000000000000000000000000000000000000000000", 'C', true
		=> Ok("SC00000000000000000000000000000000000000000000000000000000".to_string());
		"valid cluster seed")]
    #[test_case(
		"SC000000000000000000000000000000000000000000000000", 'C', true
		=> Err(ParseError::InvalidLength { found: 50, expected: 58 });
		"short cluster seed")]
    #[test_case(
		"SM00000000000000000000000000000000000000000000000000000000", 'C', true
		=> Err(ParseError::InvalidKeyType { expected: "SC".to_string(), found: "SM".to_string() });
		"cluster seed has wrong prefix")]
    #[test_case(
		"M0000000000000000000000000000000000000000000000000000000", 'M', false
		=> Ok("M0000000000000000000000000000000000000000000000000000000".to_string());
		"valid module id")]
    #[test_case(
		"M0000000000000000000000000000000000000000000000000", 'M', false
		=> Err(ParseError::InvalidLength { found: 50, expected: 56 });
		"short module id")]
    #[test_case(
		"V0000000000000000000000000000000000000000000000000000000", 'M', false
		=> Err(ParseError::InvalidKeyType { expected: "M".to_string(), found: "V".to_string() });
		"module id has wrong prefix")]
    fn test_parse(value: &str, prefix: char, is_seed: bool) -> Result<String, ParseError> {
        parse(value, prefix, is_seed)
    }

    #[test]
    fn seed_default() {
        assert_eq!(ClusterSeed::default(), Seed::<'C'>("".to_string()));
        assert_eq!(Seed::<'M'>::default(), Seed::<'M'>("".to_string()));
    }

    #[test]
    fn module_id_round_trip() {
        let a = "M0000000000000000000000000000000000000000000000000000000";
        let b = a.parse::<ModuleId>().unwrap();
        assert_eq!(a.to_string(), b.to_string());
    }

    #[test]
    fn service_id_round_trip() {
        let a = "V0000000000000000000000000000000000000000000000000000000";
        let b = a.parse::<ServiceId>().unwrap();
        assert_eq!(a.to_string(), b.to_string());
    }

    #[test]
    fn server_id_round_trip() {
        let a = "N0000000000000000000000000000000000000000000000000000000";
        let b = a.parse::<ServerId>().unwrap();
        assert_eq!(a.to_string(), b.to_string());
    }

    #[test]
    fn cluster_seed_round_trip() {
        let a = "SC00000000000000000000000000000000000000000000000000000000";
        let b = a.parse::<ClusterSeed>().unwrap();
        assert_eq!(a.to_string(), b.to_string());
    }
}
