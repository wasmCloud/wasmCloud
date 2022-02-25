pub mod client;
pub mod config;
pub mod error;

/// token to indicate string data was passed during set
pub const STRING_VALUE_MARKER: &str = "string_data___";

// generate wasmcloud_interface_keyvaule here (created by build.rs and codegen.toml)
#[allow(dead_code)]
pub mod wasmcloud_interface_keyvalue {
    include!(concat!(env!("OUT_DIR"), "/keyvalue.rs"));
}
