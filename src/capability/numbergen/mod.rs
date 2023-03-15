/// [rand](::rand) crate adaptors for random number generation capability
pub mod rand;

pub use self::rand::Numbergen as RandNumbergen;

pub use uuid::{self, Uuid};

use core::fmt::Debug;

use anyhow::{bail, Context, Error, Result};
use serde::Serialize;
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud_interface_numbergen::RangeLimit;

#[derive(Clone, Debug)]
/// Random number generator invocation
pub enum Invocation {
    /// Generates a v4 [Uuid]
    GenerateGuid,
    /// Returns a random [u32] within inclusive range from `min` to `max`
    RandomInRange {
        /// Minimum [u32] to return
        min: u32,
        /// Maximum [u32] to return
        max: u32,
    },
    /// Returns a random [u32]
    Random32,
}

impl<O, P> TryFrom<(O, Option<P>)> for Invocation
where
    O: AsRef<str>,
    P: AsRef<[u8]>,
{
    type Error = Error;

    fn try_from((operation, payload): (O, Option<P>)) -> Result<Self> {
        match operation.as_ref() {
            "NumberGen.GenerateGuid" => Ok(Self::GenerateGuid),
            "NumberGen.RandomInRange" => {
                let payload = payload.context("payload cannot be empty")?;
                let RangeLimit { min, max } =
                    deserialize(payload.as_ref()).context("failed to deserialize range limit")?;
                Ok(Self::RandomInRange { min, max })
            }
            "NumberGen.Random32" => Ok(Self::Random32),
            operation => bail!("unknown operation: `{operation}`"),
        }
    }
}

/// Serialize response to format expected by the actor
///
/// # Errors
///
/// Returns an [Error] if serialization fails
pub fn serialize_response(res: &impl Serialize) -> Result<Vec<u8>> {
    serialize(res).context("failed to serialize value")
}
