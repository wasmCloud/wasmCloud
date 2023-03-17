mod common;
use common::*;

use anyhow::Context;
use wasm_compose::graph::{self, CompositionGraph};
use wasmcloud::capability::{HandlerFunc, HostInvocation};
use wasmcloud::{Actor, Runtime};

#[derive(serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq)]
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

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
struct PublishData {
    subject: String,
    message: Vec<u8>,
}

async fn host_call(
    _claims: jwt::Claims<jwt::Actor>,
    binding: String,
    HostInvocation {
        namespace,
        operation,
        payload,
    }: HostInvocation,
) -> anyhow::Result<Option<&'static str>> {
    assert_eq!(binding, "default");
    match (namespace.as_ref(), operation.as_ref()) {
        ("wasmcloud:wasi:messaging", "Messaging.Producer.publish") => {
            let payload = payload.expect("missing payload");
            let PublishData { subject, message } =
                rmp_serde::from_slice(&payload).expect("failed to decode publish data");
            assert_eq!(subject, "rust");
            let GuestEvent {
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
            } = rmp_serde::from_slice(&message).expect("failed to decode event");
            assert_eq!(specversion, "1.0");
            assert_eq!(ty, "com.my-messaing.rust.fizzbuzz"); // note the typo
            assert_eq!(source, "rust");
            assert_eq!(id, "123");
            assert_eq!(data, Some(b"buzzbuzz".to_vec()));
            assert_eq!(datacontenttype, None);
            assert_eq!(dataschema, None);
            assert_eq!(subject, None);
            assert_eq!(time, None);
            assert_eq!(extensions, None);
            Ok(None)
        }
        ("wasmcloud:wasi:messaging", "Messaging.Producer.subscribe") => {
            println!("subscribe {payload:?}");
            Ok(Some("subscribe-token"))
        }
        ("wasmcloud:wasi:messaging", "Messaging.Producer.unsubscribe") => {
            println!("unsubscribe {payload:?}");
            Ok(None)
        }
        _ => panic!("unsupported `{namespace}.{operation}` request"),
    }
}

fn new_runtime() -> Runtime {
    Runtime::from_host_handler(HandlerFunc::from(host_call)).expect("failed to construct runtime")
}

#[tokio::test]
async fn actor_wasi_messaging_component() -> anyhow::Result<()> {
    init();

    let mut g = CompositionGraph::new();

    let host = wat::parse_file(env!(
        "CARGO_CDYLIB_FILE_ACTOR_WASI_MESSAGING_HOST_COMPONENT"
    ))
    .context("failed to parse binary")?;
    let host = encode_component_lib(&host, true).context("failed to encode `host`")?;
    let host =
        graph::Component::from_bytes("$host", host).context("failed to parse `host` component")?;

    let messaging = wat::parse_file(env!(
        "CARGO_CDYLIB_FILE_ACTOR_WASI_MESSAGING_COMPONENT_guest"
    ))
    .context("failed to parse binary")?;
    let messaging =
        encode_component_lib(&messaging, true).context("failed to encode `messaging`")?;
    let messaging = graph::Component::from_bytes("$messaging", messaging)
        .context("failed to parse `messaging` component")?;

    let guest = wat::parse_file(env!(
        "CARGO_CDYLIB_FILE_ACTOR_WASI_MESSAGING_GUEST_COMPONENT"
    ))
    .context("failed to parse binary")?;
    let guest = encode_component_lib(&guest, true).context("failed to encode `guest`")?;
    let guest = graph::Component::from_bytes("$guest", guest)
        .context("failed to parse `guest` component")?;

    let combined_export = host
        .exports()
        .find_map(|(id, name, _, _, _)| name.eq("combined").then_some(id))
        .expect("could not find `combined` export in `host`");

    // TODO: Utilize once Wasmtime allows components to export multiple interfaces
    //let producer_export = host
    //    .exports()
    //    .find_map(|(id, name, _, _, _)| name.eq("producer").then_some(id))
    //    .expect("could not find `producer` export in `host`");
    let producer_import = messaging
        .imports()
        .find_map(|(id, name, _, _)| name.eq("producer").then_some(id))
        .expect("could not find `producer` import in `messaging`");

    // TODO: Utilize once Wasmtime allows components to export multiple interfaces
    //let messaging_types_export = host
    //    .exports()
    //    .find_map(|(id, name, _, _, _)| name.eq("messaging-types").then_some(id))
    //    .expect("could not find `messaging_types` export in `host`");
    let messaging_types_import = messaging
        .imports()
        .find_map(|(id, name, _, _)| name.eq("messaging-types").then_some(id))
        .expect("could not find `messaging_types` import in `messaging`");

    // NOTE: This one is not actually used by the component
    //let consumer_export = host
    //    .exports()
    //    .find_map(|(id, name, _, _, _)| name.eq("consumer").then_some(id))
    //    .expect("could not find `consumer` export in `host`");
    //let consumer_import = messaging
    //    .imports()
    //    .find_map(|(id, name, _, _)| {
    //        tracing::error!(?id, name);
    //        name.eq("consumer").then_some(id)
    //    })
    //    .expect("could not find `consumer` import in `messaging`");

    let handler_export = messaging
        .exports()
        .find_map(|(id, name, _, _, _)| name.eq("handler").then_some(id))
        .expect("could not find `handler` export in `messaging`");
    let handler_import = guest
        .imports()
        .find_map(|(id, name, _, _)| name.eq("handler").then_some(id))
        .expect("could not find `handler` import in `guest`");

    let host = g
        .add_component(host)
        .context("failed to add `host` component to the graph")?;
    let messaging = g
        .add_component(messaging)
        .context("failed to add `messaging` component to the graph")?;
    let guest = g
        .add_component(guest)
        .context("failed to add `guest` component to the graph")?;

    let host = g
        .instantiate(host)
        .context("failed to instantiate `host`")?;
    let messaging = g
        .instantiate(messaging)
        .context("failed to instantiate `messaging`")?;
    let guest = g
        .instantiate(guest)
        .context("failed to instantiate `guest`")?;

    g.connect(
        host,
        Some(combined_export),
        messaging,
        messaging_types_import,
    )
    .context("failed to connect `combined` from `host` to `messaging`")?;
    g.connect(host, Some(combined_export), messaging, producer_import)
        .context("failed to connect `combined` from `host` to `messaging`")?;
    // NOTE: This one is not actually used by the component
    //g.connect(host, Some(consumer_export), messaging, consumer_import)
    //    .context("failed to connect `consumer` from `host` to `messaging`")?;
    g.connect(messaging, Some(handler_export), guest, handler_import)
        .context("failed to connect `handler` from `messaging` to `guest`")?;

    let wasm = g
        .encode(graph::EncodeOptions {
            define_components: true,
            export: Some(guest),
            validate: true,
        })
        .context("failed to encode graph")?;

    tokio::fs::write("/tmp/msg.wasm", &wasm)
        .await
        .expect("failed to write messaging Wasm");

    let (wasm, key) = sign(wasm, "wasi-messaging", []).context("failed to sign component")?;

    let rt = new_runtime();
    let actor = Actor::new(&rt, wasm).expect("failed to construct actor");
    assert_eq!(actor.claims().subject, key.public_key());

    let event = rmp_serde::to_vec(&GuestEvent {
        specversion: "specversion".into(),
        ty: "ty".into(),
        source: "source".into(),
        id: "id".into(),
        data: Some("fizzbuzz".into()),
        datacontenttype: Some("datacontenttype".into()),
        dataschema: Some("dataschema".into()),
        subject: Some("subject".into()),
        time: Some("time".into()),
        extensions: None,
    })
    .expect("failed to encode event");

    let response = actor
        .call("Messaging.Handler.on_receive", Some(event))
        .await
        .context("failed to call `Messaging.Handler.on_receive`")?
        .expect("`Messaging.Handler.on_receive` must not fail");
    assert_eq!(response, None);
    Ok(())
}
