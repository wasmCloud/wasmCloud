use crate::capability::Handle;

use super::{serialize_response, uuid, Invocation};

use anyhow::Result;
use async_trait::async_trait;
use rand::{CryptoRng, Rng};
use tracing::{instrument, trace};
use wascap::jwt;

/// A random number generation capability wrapping an arbitrary [`rand::Rng`] implementation.
/// Note, that the underlying random number generator MUST implement [`rand::CryptoRng`] as an
/// additional security measure.
pub struct Numbergen<T = ::rand::rngs::OsRng>(T);

impl<T> From<T> for Numbergen<T>
where
    T: Rng + CryptoRng + Sync + Send + Copy,
{
    fn from(r: T) -> Self {
        Self(r)
    }
}

impl Default for Numbergen {
    fn default() -> Self {
        Self(::rand::rngs::OsRng)
    }
}

#[async_trait]
impl<T> Handle<Invocation> for Numbergen<T>
where
    T: Rng + CryptoRng + Sync + Send + Copy,
{
    #[instrument(skip(self))]
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        _binding: String,
        invocation: Invocation,
        _call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        match invocation {
            Invocation::GenerateGuid => {
                let mut buf = uuid::Bytes::default();
                let mut rng = self.0;
                rng.fill_bytes(&mut buf);
                let guid = uuid::Builder::from_random_bytes(buf)
                    .into_uuid()
                    .to_string();
                trace!(?guid, "generated GUID");
                serialize_response(&guid)
            }

            Invocation::RandomInRange { min, max } => {
                let mut rng = self.0;
                let v = rng.gen_range(min..=max);
                trace!(v, "generated random u32 in range");
                serialize_response(&v)
            }
            Invocation::Random32 => {
                let mut rng = self.0;
                let v: u32 = rng.gen();
                trace!(v, "generated random u32");
                serialize_response(&v)
            }
        }
        .map(Some)
    }
}
