mod bindings {
    wit_bindgen::generate!({
        world: "component",
        generate_all
    });
}

use bindings::exports::wasmcloud::example::reader::Guest;
use bindings::wasi::config::store;

struct Component;

impl Guest for Component {
    fn read() -> String {
        match store::get_all() {
            Ok(mut entries) => {
                entries.sort();
                entries
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(";")
            }
            Err(e) => format!("error:{e:?}"),
        }
    }
}

bindings::export!(Component with_types_in bindings);
