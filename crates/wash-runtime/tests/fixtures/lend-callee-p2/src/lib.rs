//! P2 fixture, served from a store of its own. Uses the host resources its
//! caller hands it: streams, header fields and a whole request.

mod bindings {
    wit_bindgen::generate!({
        world: "callee",
        generate_all,
    });
}

use bindings::exports::wasmcloud::lend_test::relay::Guest;
use bindings::wasi::http::types::{Fields, IncomingRequest, Method};
use bindings::wasi::io::streams::{InputStream, OutputStream};

struct Component;

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Everything left in `data`. A read fails once the stream has ended.
fn read_to_end(data: &InputStream) -> String {
    let mut bytes = Vec::new();
    while let Ok(chunk) = data.blocking_read(4096) {
        bytes.extend(chunk);
    }
    text(bytes)
}

fn values(headers: &Fields, name: &str) -> String {
    let values = headers.get(name).into_iter().map(text);
    values.collect::<Vec<_>>().join(",")
}

impl Guest for Component {
    fn peek(data: &InputStream, len: u64) -> String {
        text(data.blocking_read(len).unwrap_or_default())
    }

    fn drain(data: InputStream) -> String {
        read_to_end(&data)
    }

    fn pass(data: InputStream) -> InputStream {
        data
    }

    fn write(out: &OutputStream, text: String) {
        out.blocking_write_and_flush(text.as_bytes()).unwrap();
    }

    fn finish(out: OutputStream, text: String) {
        out.blocking_write_and_flush(text.as_bytes()).unwrap();
    }

    fn header(headers: &Fields, name: String) -> String {
        values(headers, &name)
    }

    fn both(first: &Fields, second: &Fields, name: String) -> String {
        format!("{}+{}", values(first, &name), values(second, &name))
    }

    fn describe(request: IncomingRequest) -> String {
        let method = match request.method() {
            Method::Get => "GET",
            Method::Post => "POST",
            _ => "other",
        };
        let path = request.path_with_query().unwrap_or_default();
        let body = request.consume().unwrap();
        let data = body.stream().unwrap();
        format!("{method} {path} {}", read_to_end(&data))
    }
}

bindings::export!(Component with_types_in bindings);
