#![cfg(target_arch = "wasm32")]

wit_bindgen::generate!({
    world: "foobar",
    path: "../wit",
});

struct Foobar;

impl foobar_guest::FoobarGuest for Foobar {
    fn foobar() -> String {
        println!("Hello from wrapped Foobar");

        format!("{}bar", foobar_host::foo())
    }
}

export_foobar!(Foobar);
