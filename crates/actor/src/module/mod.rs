pub mod host;
pub mod http;
pub mod logging;
pub mod random;

pub use wasmcloud_actor_derive::*;

pub use http::{Handler as HttpHandler, Request as HttpRequest, Response as HttpResponse};

pub trait Handler<T: ?Sized> {
    type Error: ToString;

    fn handle(&self, operation: &str, payload: Vec<u8>) -> Option<Result<Vec<u8>, Self::Error>>;
}
