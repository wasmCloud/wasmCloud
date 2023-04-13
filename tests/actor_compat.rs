mod common;
use common::*;

use anyhow::Context;
use test_actors::encode_component;
use wasm_compose::graph::{self, CompositionGraph};
use wasmcloud::capability::{HandlerFunc, HostInvocation};
use wasmcloud::{Actor, Runtime};

async fn host_call(
    _claims: jwt::Claims<jwt::Actor>,
    _binding: String,
    HostInvocation {
        namespace,
        operation,
        payload,
    }: HostInvocation,
    call_context: Option<Vec<u8>>,
) -> anyhow::Result<Option<&'static str>> {
    assert_eq!(namespace, "FoobarHost");
    assert_eq!(operation, "Foobar.Foo");
    assert_eq!(payload, None);
    assert_eq!(call_context, None);
    Ok(Some("foo"))
}

fn new_runtime() -> Runtime {
    Runtime::from_host_handler(HandlerFunc::from(host_call)).expect("failed to construct runtime")
}

#[tokio::test]
async fn actor_compat_component() -> anyhow::Result<()> {
    init();

    let mut g = CompositionGraph::new();

    let host = encode_component(test_actors::RUST_FOOBAR_HOST_COMPONENT, true)
        .context("failed to encode `host`")?;
    let host =
        graph::Component::from_bytes("$host", host).context("failed to parse `host` component")?;

    let foobar = encode_component(test_actors::RUST_FOOBAR_COMPONENT, true)
        .context("failed to encode `foobar`")?;
    let foobar = graph::Component::from_bytes("$foobar", foobar)
        .context("failed to parse `foobar` component")?;

    let guest = encode_component(test_actors::RUST_FOOBAR_GUEST_COMPONENT, true)
        .context("failed to encode `guest`")?;
    let guest = graph::Component::from_bytes("$guest", guest)
        .context("failed to parse `foobar-guest` component")?;

    let host_export = host
        .exports()
        .find_map(|(id, name, _, _, _)| name.eq("foobar-host").then_some(id))
        .expect("could not find `foobar-guest` export in `foobar`");
    let foobar_import = foobar
        .imports()
        .find_map(|(id, name, _, _)| name.eq("foobar-host").then_some(id))
        .expect("could not find `foobar-host` import in `foobar`");

    let foobar_export = foobar
        .exports()
        .find_map(|(id, name, _, _, _)| name.eq("foobar-guest").then_some(id))
        .expect("could not find `foobar-guest` export in `foobar`");
    let guest_import = guest
        .imports()
        .find_map(|(id, name, _, _)| name.eq("foobar-guest").then_some(id))
        .expect("could not find `foobar-guest` import in `foobar`");

    let host = g
        .add_component(host)
        .context("failed to add `foobar-host` component to the graph")?;
    let foobar = g
        .add_component(foobar)
        .context("failed to add `foobar` component to the graph")?;
    let guest = g
        .add_component(guest)
        .context("failed to add `foobar-guest` component to the graph")?;

    let host = g
        .instantiate(host)
        .context("failed to instantiate `foobar-host`")?;
    let foobar = g
        .instantiate(foobar)
        .context("failed to instantiate `foobar`")?;
    let guest = g
        .instantiate(guest)
        .context("failed to instantiate `foobar-guest`")?;

    g.connect(host, Some(host_export), foobar, foobar_import)
        .context("failed to connect `foobar-host` from `host` to `foobar`")?;
    g.connect(foobar, Some(foobar_export), guest, guest_import)
        .context("failed to connect `foobar-guest` from `foobar` to `guest`")?;

    let wasm = g
        .encode(graph::EncodeOptions {
            define_components: true,
            export: Some(guest),
            validate: true,
        })
        .context("failed to encode graph")?;
    let (wasm, key) = sign(wasm, "compat", []).context("failed to sign component")?;

    let rt = new_runtime();
    let actor = Actor::new(&rt, wasm).expect("failed to construct actor");
    assert_eq!(actor.claims().subject, key.public_key());

    let response = actor
        .call("FoobarGuest.Foobar", None::<Vec<u8>>)
        .await
        .context("failed to call `FoobarGuest.Foobar`")?
        .expect("`FoobarGuest.Foobar` must not fail");
    assert_eq!(response, Some("foobar".into()));
    Ok(())
}
