#[cfg(feature = "rand")]
pub use rand::{Rng, RngCore};

#[cfg(feature = "uuid")]
pub use uuid::Uuid;

pub struct HostRng;

impl HostRng {
    /// Generate a 32-bit random number
    #[inline]
    pub fn random32() -> u32 {
        crate::wasi::random::random::get_random_u64() as _
    }

    /// Generate a v4-format guid in the form "nnnnnnnn-nnnn-nnnn-nnnn-nnnnnnnnnnnn"
    /// where n is a lowercase hex digit and all bits are random.
    #[cfg(feature = "uuid")]
    pub fn generate_guid() -> Uuid {
        let buf = uuid::Bytes::try_from(crate::wasi::random::random::get_random_bytes(16))
            .expect("invalid amount of bytes generated");
        uuid::Builder::from_random_bytes(buf).into_uuid()
    }

    /// Generate a random integer within an inclusive range. ( min <= n <= max )
    #[cfg(feature = "rand")]
    pub fn random_in_range(min: u32, max: u32) -> u32 {
        HostRng.gen_range(min..=max)
    }
}

#[cfg(feature = "rand")]
impl crate::RngCore for HostRng {
    #[inline]
    fn next_u32(&mut self) -> u32 {
        HostRng::random32()
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        crate::wasi::random::random::get_random_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let n = dest.len();
        if usize::BITS <= u64::BITS || n <= u64::MAX as _ {
            dest.copy_from_slice(&crate::wasi::random::random::get_random_bytes(n as _));
        } else {
            let (head, tail) = dest.split_at_mut(u64::MAX as _);
            head.copy_from_slice(&crate::wasi::random::random::get_random_bytes(u64::MAX));
            // TODO: Optimize
            self.fill_bytes(tail);
        }
    }

    #[inline]
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
