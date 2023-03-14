#![cfg(feature = "rand")]

use super::{uuid, Uuid};

use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use rand::{CryptoRng, Rng};
use tracing::{instrument, trace};
use wascap::jwt;

/// A random number generation capability wrapping an arbitrary [`rand::Rng`] implementation.
/// Note, that the underlying random number generator MUST implement [`rand::CryptoRng`] as an
/// additional security measure.
pub struct Numbergen<T>(Mutex<T>);

impl<T: Rng + CryptoRng> From<T> for Numbergen<T> {
    fn from(r: T) -> Self {
        Self(r.into())
    }
}

impl<T> Numbergen<T> {
    fn lock(&self) -> Result<MutexGuard<T>, &'static str> {
        self.0.lock().map_err(|_| "RNG not available")
    }
}

#[async_trait]
impl<T: Rng + CryptoRng + Sync + Send> super::Numbergen for Numbergen<T> {
    type Error = &'static str;

    #[instrument(skip(self))]
    async fn generate_guid(&self, _: &jwt::Claims<jwt::Actor>) -> Result<Uuid, Self::Error> {
        let mut buf = uuid::Bytes::default();
        self.lock()?.fill_bytes(&mut buf);
        let guid = uuid::Builder::from_random_bytes(buf).into_uuid();
        trace!(?guid, "generated GUID");
        Ok(guid)
    }

    #[instrument(skip(self))]
    async fn random_in_range(
        &self,
        _: &jwt::Claims<jwt::Actor>,
        min: u32,
        max: u32,
    ) -> Result<u32, Self::Error> {
        let v = self.lock()?.gen_range(min..=max);
        trace!(v, "generated random u32 in range");
        Ok(v)
    }

    #[instrument(skip(self))]
    async fn random_32(&self, _: &jwt::Claims<jwt::Actor>) -> Result<u32, Self::Error> {
        let v = self.lock()?.gen();
        trace!(v, "generated random u32");
        Ok(v)
    }
}
