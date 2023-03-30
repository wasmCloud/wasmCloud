wit_bindgen::generate!("interfaces");

#[cfg(feature = "rand")]
pub use rand::{Rng, RngCore};

#[cfg(feature = "uuid")]
pub use uuid::Uuid;

pub struct HostRng;

impl HostRng {
    #[inline]
    pub fn random32() -> u32 {
        random::get_random_u64() as _
    }

    #[cfg(feature = "uuid")]
    pub fn generate_guid() -> Uuid {
        let buf = uuid::Bytes::try_from(random::get_random_bytes(16))
            .expect("invalid amount of bytes generated");
        uuid::Builder::from_random_bytes(buf).into_uuid()
    }

    #[cfg(feature = "rand")]
    pub fn random_in_range(min: u32, max: u32) -> u32 {
        HostRng.gen_range(min..=max)
    }
}

#[cfg(feature = "rand")]
impl RngCore for HostRng {
    #[inline]
    fn next_u32(&mut self) -> u32 {
        HostRng::random32()
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        random::get_random_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let n = dest.len();
        if usize::BITS <= u64::BITS || n <= u64::MAX as _ {
            dest.copy_from_slice(&random::get_random_bytes(n as _));
        } else {
            let (head, tail) = dest.split_at_mut(u64::MAX as _);
            head.copy_from_slice(&random::get_random_bytes(u64::MAX));
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

#[cfg(test)]
#[allow(dead_code)]
mod test {
    use super::*;

    struct Actor;

    impl Actor {
        fn use_host_exports() {
            logging::log(logging::Level::Trace, "context", "message");
            logging::log(logging::Level::Debug, "context", "message");
            logging::log(logging::Level::Info, "context", "message");
            logging::log(logging::Level::Warn, "context", "message");
            logging::log(logging::Level::Error, "context", "message");
            random::get_random_bytes(4);
            random::get_random_u64();
            random::insecure_random();
            // TODO: Add support for HTTP
            //outgoing_http::handle(
            //    types::new_outgoing_request(
            //        types::MethodParam::Get,
            //        "path",
            //        "query",
            //        Some(types::SchemeParam::Https),
            //        "authority",
            //        types::new_fields(&[("myheader", "myvalue")]),
            //    ),
            //    Some(types::RequestOptions {
            //        connect_timeout_ms: Some(42),
            //        first_byte_timeout_ms: Some(42),
            //        between_bytes_timeout_ms: Some(42),
            //    }),
            //);
            host::call("binding", "namespace", "operation", Some(b"payload")).unwrap();
        }
    }
}
