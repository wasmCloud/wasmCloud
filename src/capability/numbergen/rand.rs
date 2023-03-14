#![cfg(feature = "rand")]

use super::{uuid, Uuid};

use std::sync::{Mutex, MutexGuard};

use rand::{CryptoRng, Rng};
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

impl<T: Rng + CryptoRng> super::Numbergen for Numbergen<T> {
    type Error = &'static str;

    fn generate_guid(&self, _: &jwt::Claims<jwt::Actor>) -> Result<Uuid, Self::Error> {
        let mut buf = uuid::Bytes::default();
        self.lock()?.fill_bytes(&mut buf);
        Ok(uuid::Builder::from_random_bytes(buf).into_uuid())
    }

    fn random_in_range(
        &self,
        _: &jwt::Claims<jwt::Actor>,
        min: u32,
        max: u32,
    ) -> Result<u32, Self::Error> {
        let v = self.lock()?.gen_range(min..=max);
        Ok(v)
    }

    fn random_32(&self, _: &jwt::Claims<jwt::Actor>) -> Result<u32, Self::Error> {
        let v = self.lock()?.gen();
        Ok(v)
    }
}
