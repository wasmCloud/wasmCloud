wit_bindgen::generate!({
    world: "guest",
    path: "../wit",
});

use handler::Event;

struct Guest;

#[derive(serde::Deserialize)]
struct PubMessage {
    subject: String,
    message: Vec<u8>,
}

#[derive(serde::Deserialize, Debug)]
struct GuestEvent {
    specversion: String,
    ty: String,
    source: String,
    id: String,
    data: Option<Vec<u8>>,
    datacontenttype: Option<String>,
    dataschema: Option<String>,
    subject: Option<String>,
    time: Option<String>,
    extensions: Option<Vec<(String, String)>>,
}

impl actor::Actor for Guest {
    fn guest_call(operation: String, payload: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
        match operation.as_ref() {
            // op name is world.interface.method
            "Messaging.Handler.on_receive" => {
                let payload = payload.unwrap_or_default();
                let rpc = rmp_serde::from_slice::<PubMessage>(&payload)
                    .map_err(|e| format!("deserializing {}: {e}", &operation))?;
                let event = serde_json::from_slice::<GuestEvent>(&rpc.message)
                    .map_err(|e| format!("deserializing Event: {e}"))?;
                let res = Guest::on_receive(event);
                if let Err(e) = res {
                    Err(format!("on_receive returned {e}"))
                } else {
                    Ok(None)
                }
            }
            _ => {
                let msg = "invalid invocation on adapter. Expecting Handler.on_receive";
                println!("{msg}");
                Err(msg.to_string())
            }
        }
    }
}

/// Handle subscription callbacks (CloudEvent delivery on subscribed channel)
impl Guest {
    /// Receive from host, convert wasmbus-rpc to wit object, forward to downstream
    fn on_receive(e: GuestEvent) -> Result<(), u32> {
        println!(">>> Received: {:#?}", e);
        let param = Event {
            specversion: &e.specversion,
            ty: &e.ty,
            source: &e.source,
            id: &e.id,
            data: e.data.as_deref(),
            datacontenttype: e.datacontenttype.as_deref(),
            dataschema: e.dataschema.as_deref(),
            subject: e.subject.as_deref(),
            time: e.time.as_deref(),
            extensions: None,
        };
        // forward to downstream
        // there's no 'self' param so we just send to static function export
        let res = handler::on_receive(param);
        if let Err(e) = res {
            println!("error forwarding event: {}", e.to_string());
            Err(e)
        } else {
            Ok(())
        }
    }
}

export_guest!(Guest);
