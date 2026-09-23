mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let headers = Fields::new();
        let path = request.get_path_with_query().unwrap_or_default();
        let is_bulk = path.starts_with("/bulk");
        let is_stream = path.starts_with("/stream");

        let (mut tx, rx) = bindings::wit_stream::new();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        wit_bindgen::spawn_local(async move {
            if is_bulk {
                let chunk = vec![b'x'; 65536];
                for _ in 0..16 {
                    tx.write_all(chunk.clone()).await;
                }
            } else if is_stream {
                let chunk = b"hello from p3".to_vec();
                for _ in 0..500 {
                    tx.write_all(chunk.clone()).await;
                }
            } else {
                tx.write_all(b"hello from p3".to_vec()).await;
            }
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
