use derive_more::{Display, From, Into};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IdParseError {
    #[error(r#"found the prefix "{found}", but expected "{expected}""#)]
    WrongKeyType { found: char, expected: char },
    #[error("found length {0}, but should be 56 chars")]
    WrongLength(usize),
    #[error("unknown parse error")]
    Unknown,
}

#[derive(Clone, Debug, Display, PartialEq, From, Into, Serialize, Deserialize)]
pub struct Id<const PREFIX: char>(String);

impl<const PREFIX: char> FromStr for Id<PREFIX> {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let count = s.chars().count();
        if count != 56 {
            return Err(IdParseError::WrongLength(count));
        }

        if s.starts_with(PREFIX) {
            Ok(Self(s.to_string()))
        } else {
            let prefix = s
                .chars()
                .next()
                .expect("we already know it's the right length");
            Err(IdParseError::WrongKeyType {
                found: prefix,
                expected: PREFIX,
            })
        }
    }
}

pub type ModuleId = Id<'M'>;
pub type ServerId = Id<'N'>;
pub type ServiceId = Id<'V'>;
