extern crate rmp_serde as rmps;
use rmps::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

extern crate log;
//extern crate wapc_guest as guest;
//use guest::prelude::*;

use lazy_static::lazy_static;
use std::sync::RwLock;

pub struct Host {
    binding: String,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            binding: "default".to_string(),
        }
    }
}

/// Creates a named host binding for the key-value store capability
pub fn host(binding: &str) -> Host {
    Host {
        binding: binding.to_string(),
    }
}

/// Creates the default host binding for the key-value store capability
pub fn default() -> Host {
    Host::default()
}

impl Host {}

pub struct Handlers {}

impl Handlers {}

lazy_static! {}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct GeneratorResult {
    #[serde(rename = "guid")]
    pub guid: Option<String>,
    #[serde(rename = "sequenceNumber")]
    pub sequence_number: u64,
    #[serde(rename = "random_number")]
    pub random_number: u32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct GeneratorRequest {
    #[serde(rename = "guid")]
    pub guid: bool,
    #[serde(rename = "sequence")]
    pub sequence: bool,
    #[serde(rename = "random")]
    pub random: bool,
    #[serde(rename = "min")]
    pub min: u32,
    #[serde(rename = "max")]
    pub max: u32,
}

/// The standard function for serializing codec structs into a format that can be
/// used for message exchange between actor and host. Use of any other function to
/// serialize could result in breaking incompatibilities.
pub(crate) fn serialize<T>(
    item: T,
) -> ::std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
where
    T: Serialize,
{
    let mut buf = Vec::new();
    item.serialize(&mut Serializer::new(&mut buf).with_struct_map())?;
    Ok(buf)
}

/// The standard function for de-serializing codec structs from a format suitable
/// for message exchange between actor and host. Use of any other function to
/// deserialize could result in breaking incompatibilities.
pub(crate) fn deserialize<'de, T: Deserialize<'de>>(
    buf: &[u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut de = Deserializer::new(Cursor::new(buf));
    match Deserialize::deserialize(&mut de) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("Failed to de-serialize: {}", e).into()),
    }
}
