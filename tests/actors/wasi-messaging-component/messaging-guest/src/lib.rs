#![cfg(target_arch = "wasm32")]

wit_bindgen::generate!({
    world: "guest",
    path: "../wit",
});

use handler::Event;

struct Guest;

impl actor::Actor for Guest {
    fn guest_call(operation: String, payload: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
        assert_eq!(operation, "Handler.OnReceive");
        handler::on_receive(Event {
            specversion: "specversion",
            ty: "ty",
            source: "source",
            id: "id",
            data: payload.as_ref().map(Vec::as_slice),
            datacontenttype: Some("datacontenttype"),
            dataschema: Some("dataschema"),
            subject: Some("subject"),
            time: Some("time"),
            extensions: None,
        })
        .expect("failed to send event");
        Ok(None)
    }
}

export_guest!(Guest);
