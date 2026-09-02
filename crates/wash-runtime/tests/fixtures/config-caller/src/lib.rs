mod bindings {
    wit_bindgen::generate!({
        world: "component",
        generate_all
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::config::store,
    wasi::http::types::{
        Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
    },
    wasmcloud::example::reader,
};

struct Component;

fn own_config() -> String {
    match store::get_all() {
        Ok(mut entries) => {
            entries.sort();
            entries
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(";")
        }
        Err(e) => format!("error:{e:?}"),
    }
}

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let payload = format!("caller[{}] callee[{}]", own_config(), reader::read());

        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let stream = body.write().unwrap();
        stream.blocking_write_and_flush(payload.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(body, None).unwrap();
    }
}

bindings::export!(Component with_types_in bindings);
