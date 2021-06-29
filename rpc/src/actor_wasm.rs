//! imports/exports for WebAssembly actors
#![cfg(target_arch = "wasm32")]

use crate::{client, context, Message, RpcError, Transport};
use async_trait::async_trait;

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
    //pub fn __guest_response(ptr: *const u8, len: usize);
    //pub fn __guest_error(ptr: *const u8, len: usize);
    //pub fn __guest_request(op_ptr: *const u8, ptr: *const u8);
}

/// The function through which all host calls (from actors) take place.
pub fn host_call(
    binding: &str,
    ns: &str,
    op: &str,
    msg: &[u8],
) -> std::result::Result<Vec<u8>, RpcError> {
    let callresult = unsafe {
        __host_call(
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
        let errlen = unsafe { __host_error_len() };
        let buf = Vec::with_capacity(errlen as _);
        let retptr = buf.as_ptr();
        let slice = unsafe {
            __host_error(retptr);
            std::slice::from_raw_parts(retptr as _, errlen as _)
        };
        Err(RpcError::HostError(
            String::from_utf8_lossy(&slice.to_vec()).to_string(),
        ))
    } else {
        // call succeeded
        let len = unsafe { __host_response_len() };
        let buf = Vec::with_capacity(len as _);
        let retptr = buf.as_ptr();
        let slice = unsafe {
            __host_response(retptr);
            std::slice::from_raw_parts(retptr as _, len as _)
        };
        Ok(slice.to_vec())
    }
}

#[derive(Clone, Debug, Default)]
pub struct WasmHost {}

#[async_trait]
impl Transport for WasmHost {
    async fn send(
        &self,
        _ctx: &context::Context<'_>,
        config: &client::SendConfig,
        req: Message<'_>,
    ) -> std::result::Result<Message<'static>, RpcError> {
        let res = host_call(
            &config.host,   // "default", or capability provider ID
            &config.target, // actor_ref, or capability name (e.g. wasmcloud::messaging)
            req.method,
            req.arg.as_ref(),
        )?;
        Ok(Message {
            method: "_reply",
            arg: std::borrow::Cow::Owned(res),
        })
    }
}

#[cold]
#[inline(never)]
pub fn console_log(s: &str) {
    unsafe {
        __console_log(s.as_ptr(), s.len());
    }
}
