#![cfg(target_arch = "wasm32")]

wit_bindgen::generate!({
    world: "host",
    path: "../wit",
});

struct Host;

impl foobar_host::FoobarHost for Host {
    fn foo() -> String {
        let buf = host::host_call("default", "FoobarHost", "Foobar.Foo", None)
            .expect("failed to `foo`")
            .expect("missing payload");
        let s = String::from_utf8(buf).expect("failed to parse `foo`");
        assert_eq!(s, "foo");
        s
    }
}

export_host!(Host);
