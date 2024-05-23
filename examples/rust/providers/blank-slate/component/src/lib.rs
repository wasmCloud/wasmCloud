wit_bindgen::generate!();

use crate::exports::wasmcloud::example::process_data::Data;
use crate::exports::wasmcloud::example::process_data::Guest;
use crate::wasi::logging::logging::*;
use crate::wasmcloud::example::system_info::Kind;

struct BlankSlateComponent;

impl Guest for BlankSlateComponent {
    fn process(data: Data) -> String {
        log(Level::Info, "", &format!("Data received: {:?}", data));
        // Request OS and architecture information
        let os = crate::wasmcloud::example::system_info::request_info(Kind::Os);
        let arch = crate::wasmcloud::example::system_info::request_info(Kind::Arch);
        format!("Provider is running on {os}-{arch}").to_string()
    }
}

export!(BlankSlateComponent);
