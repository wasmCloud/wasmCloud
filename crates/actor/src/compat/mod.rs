pub mod wasi;
pub mod wasmcloud;

pub mod http; // TODO: This should have a component model counterpart
pub use http::{Handler as HttpHandler, Request as HttpRequest, Response as HttpResponse};

pub trait Handler<T: ?Sized> {
    type Error: ToString;

    fn handle(&self, operation: &str, payload: Vec<u8>) -> Option<Result<Vec<u8>, Self::Error>>;
}
