mod bindings {
    wit_bindgen::generate!({
        world: "component",
        generate_all,
    });
}

struct Component;

impl bindings::exports::wasmcloud::example::receiver::Guest for Component {
    fn invoke() -> Result<(), String> {
        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);
