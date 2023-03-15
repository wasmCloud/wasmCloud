#![cfg(target_arch = "wasm32")]

wit_bindgen::generate!({
    world: "guest",
    path: "../wit",
});

struct Guest;

impl actor::Actor for Guest {
    fn guest_call(operation: String, payload: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
        assert_eq!(payload, None);
        assert_eq!(operation, "FoobarGuest.Foobar");
        Ok(Some(foobar_guest::foobar().into()))
    }
}

export_guest!(Guest);
