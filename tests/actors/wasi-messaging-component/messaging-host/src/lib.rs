#![cfg(target_arch = "wasm32")]

wit_bindgen::generate!({
    world: "host",
    path: "../wit",
});

struct Host;

#[derive(serde::Serialize)]
struct PublishData {
    subject: String,
    message: Vec<u8>,
}

#[derive(serde::Serialize)]
enum Channel {
    Queue(String),
    Topic(String),
}

#[derive(serde::Serialize, Debug)]
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

impl combined::Combined for Host {
    fn publish(
        b: combined::Broker,
        c: combined::Channel,
        combined::Event {
            specversion,
            ty,
            source,
            id,
            data,
            datacontenttype,
            dataschema,
            subject,
            time,
            extensions,
        }: combined::Event,
    ) -> Result<(), combined::Error> {
        println!(">>> called publish");
        let event = GuestEvent {
            specversion,
            ty,
            source,
            id,
            data,
            datacontenttype,
            dataschema,
            subject,
            time,
            extensions,
        };
        let message = serde_json::to_vec(&event).map_err(|e| {
            println!("serialization of event: {e}");
            1u32
        })?;
        let rpc_message = PublishData {
            subject: match c {
                combined::Channel::Queue(s) => s,
                combined::Channel::Topic(s) => s,
            },
            message,
        };
        let vec = rmp_serde::to_vec(&rpc_message).unwrap();
        host::host_call(
            "default",                    // link name
            "wasmcloud:wasi:messaging",   // contract_id
            "Messaging.Producer.publish", // method
            Some(&vec),
        )
        .map_err(|e| {
            // TODO: is this number supposed to be a pointer?
            println!("publish error: {e}");
            1u32
        })?;
        println!("publish: len {}", vec.len(),);
        Ok(())
    }

    fn subscribe(
        b: combined::Broker,
        c: combined::Channel,
    ) -> Result<combined::SubscriptionToken, combined::Error> {
        println!(">>> called subscribe, channel: {:?}", &c);
        let c = match c {
            combined::Channel::Queue(s) => Channel::Queue(s),
            combined::Channel::Topic(s) => Channel::Topic(s),
        };

        let vec1 = rmp_serde::to_vec(&c).unwrap();
        println!("subscribe: serde encode: {}", vec1.len(),);
        let ret = host::host_call(
            "default",                      // link name
            "wasmcloud:wasi:messaging",     // contract_id
            "Messaging.Consumer.subscribe", // method
            Some(&vec1),
        )
        .map_err(|e| {
            // TODO: what is the subscribe error?
            println!("subscribe error: {e}");
            1u32
        })?
        .unwrap_or_default();
        // on success, returns a String subscribe-token
        let s = String::from_utf8_lossy(&ret);
        Ok(s.to_string())
    }

    fn unsubscribe(
        b: combined::Broker,
        st: combined::SubscriptionToken,
    ) -> Result<(), combined::Error> {
        println!(">>> called unsubscribe, token: {:?}", &st);
        let vec1 = rmp_serde::to_vec(&st).unwrap();
        println!("unsubscribe: serde encode: {}", vec1.len(),);
        let _ = host::host_call(
            "default",                        // link name
            "wasmcloud:wasi:messaging",       // contract_id
            "Messaging.Consumer.unsubscribe", // method
            Some(&vec1),
        )
        .map_err(|e| {
            println!("unsubscribe error: {e}");
            0u32
        })?;
        Ok(())
    }

    fn trace(e: combined::Error) -> String {
        format!("Error code {e}")
    }

    fn drop_error(e: combined::Error) {
        println!("drop error: {e}")
    }

    fn open_broker(_name: String) -> Result<combined::Broker, combined::Error> {
        Ok(42)
    }

    fn drop_broker(b: combined::Broker) {
        assert_eq!(b, 42);
    }
}

export_host!(Host);
