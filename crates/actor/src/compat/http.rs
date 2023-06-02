pub use wasmcloud_interface_httpserver::{HttpRequest as Request, HttpResponse as Response};

pub trait Handler {
    fn handle_request(&self, req: Request) -> Result<Response, String>;
}

impl<T: Handler> super::Handler<dyn Handler> for T {
    type Error = String;

    fn handle(&self, operation: &str, payload: Vec<u8>) -> Option<Result<Vec<u8>, Self::Error>> {
        match operation {
            "HttpServer.HandleRequest" => {
                let res = match rmp_serde::from_slice(payload.as_ref()) {
                    Ok(req) => self.handle_request(req),
                    Err(e) => return Some(Err(format!("failed to deserialize request: {e}"))),
                };
                let res = match res {
                    Ok(res) => rmp_serde::to_vec(&res),
                    Err(e) => return Some(Err(e.to_string())),
                };
                match res {
                    Ok(res) => Some(Ok(res)),
                    Err(e) => Some(Err(format!("failed to serialize response: {e}"))),
                }
            }
            _ => None,
        }
    }
}
