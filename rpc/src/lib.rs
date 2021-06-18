//! Wasmcloud Weld runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!

mod common;
pub use common::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
    WasmHost,
};
/// Code generation
/// Template rendering, for code generation
//#[cfg(not(target_arch = "wasm32"))]
//pub mod render;

/// Version number of this api
#[doc(hidden)]
pub const WELD_API_VERSION: u32 = 0; // api version 0 is binary compatible with wapc

/// This crate's published version
pub const WELD_CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type CallResult = std::result::Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>>;
pub type HandlerResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;
pub type TomlMap = toml::value::Map<String, toml::value::Value>;

pub mod actor {

    pub mod prelude {
        pub use crate::common::{client, context, Message, MessageDispatch, RpcError, WasmHost};
        // re-export async_trait
        pub use async_trait::async_trait;
        pub use wasmcloud_weld_macros::Actor;

        //#[cfg(target_arch = "wasm32")]
        #[link(wasm_import_module = "wapc")]
        extern "C" {
            pub fn __console_log(ptr: *const u8, len: usize);
            pub fn __host_call(
                bd_ptr: *const u8,
                bd_len: usize,
                ns_ptr: *const u8,
                ns_len: usize,
                op_ptr: *const u8,
                op_len: usize,
                ptr: *const u8,
                len: usize,
            ) -> usize;
            pub fn __host_response(ptr: *const u8);
            pub fn __host_response_len() -> usize;
            pub fn __host_error_len() -> usize;
            pub fn __host_error(ptr: *const u8);
            pub fn __guest_response(ptr: *const u8, len: usize);
            pub fn __guest_error(ptr: *const u8, len: usize);
            pub fn __guest_request(op_ptr: *const u8, ptr: *const u8);
        }
    }
}

pub mod provider {

    pub mod prelude {
        pub use crate::{client, context, Message, MessageDispatch, RpcError, WasmHost};
        pub use async_trait::async_trait;
        pub use wasmcloud_weld_macros::Provider;
    }
}

/// The function through which all host calls (from actors) take place.
//#[cfg(target_arch = "wasm32")]
pub fn host_call(
    binding: &str,
    ns: &str,
    op: &str,
    msg: &[u8],
) -> std::result::Result<Vec<u8>, RpcError> {
    let callresult = unsafe {
        actor::prelude::__host_call(
            binding.as_ptr() as _,
            binding.len() as _,
            ns.as_ptr() as _,
            ns.len() as _,
            op.as_ptr() as _,
            op.len() as _,
            msg.as_ptr() as _,
            msg.len() as _,
        )
    };
    if callresult != 1 {
        // call was not successful
        let errlen = unsafe { actor::prelude::__host_error_len() };
        let buf = Vec::with_capacity(errlen as _);
        let retptr = buf.as_ptr();
        let slice = unsafe {
            actor::prelude::__host_error(retptr);
            std::slice::from_raw_parts(retptr as _, errlen as _)
        };
        Err(crate::common::RpcError::HostError(
            String::from_utf8_lossy(&slice.to_vec()).to_string(),
        ))
    } else {
        // call succeeded
        let len = unsafe { actor::prelude::__host_response_len() };
        let buf = Vec::with_capacity(len as _);
        let retptr = buf.as_ptr();
        let slice = unsafe {
            actor::prelude::__host_response(retptr);
            std::slice::from_raw_parts(retptr as _, len as _)
        };
        Ok(slice.to_vec())
    }
}
