//! One of two siblings exporting the same `example:feeds/reader`
//! interface. It answers with its own component name so a caller importing the
//! interface twice under `(implements ..)` labels can prove which sibling each
//! label actually reached.

mod bindings {
    wit_bindgen::generate!({
        world: "component",
        generate_all,
    });
}

struct Component;

impl bindings::exports::example::feeds::reader::Guest for Component {
    fn source() -> String {
        "feed-a".to_string()
    }
}

bindings::export!(Component with_types_in bindings);
