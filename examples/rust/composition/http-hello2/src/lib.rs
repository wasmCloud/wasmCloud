wit_bindgen::generate!({ generate_all });

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

struct HttpServer;

impl Guest for HttpServer {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();
        let name = match request
            .path_with_query()
            .unwrap()
            .split('=')
            .collect::<Vec<&str>>()[..]
        {
            // query string is "/?name=<name>" e.g. localhost:8000?name=Bob
            ["/?name", name] => name.to_string(),
            // query string is anything else or empty e.g. localhost:8000
            _ => "World".to_string(),
        };

        let pong = example::pong::pingpong::ping();

        response_body
            .write()
            .unwrap()
            .blocking_write_and_flush(format!("Hello {name}! I got pong {pong}\n").as_bytes())
            .unwrap();
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}

export!(HttpServer);
