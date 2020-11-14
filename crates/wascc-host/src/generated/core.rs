extern crate rmp_serde as rmps;
use rmps::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

extern crate log;
//extern crate wapc_guest as guest;
//use guest::prelude::*;

use lazy_static::lazy_static;
use std::collections::HashMap;
use std::sync::RwLock;

pub struct Host {
    link: String,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            link: "default".to_string(),
        }
    }
}

/// Creates a named host link for the key-value store capability
pub fn host(link: &str) -> Host {
    Host {
        link: link.to_string(),
    }
}

/// Creates the default host link for the key-value store capability
pub fn default() -> Host {
    Host::default()
}

impl Host {}

pub struct Handlers {}

impl Handlers {}

lazy_static! {}

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct CapabilityConfiguration {
    pub module: String,
    #[serde(default)]
    pub values: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct HealthRequest {
    pub placeholder: bool,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct HealthResponse {
    pub healthy: bool,
    pub message: String,
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
