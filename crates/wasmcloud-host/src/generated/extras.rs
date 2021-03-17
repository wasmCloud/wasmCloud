use serde::{Deserialize, Serialize};

extern crate log;

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
