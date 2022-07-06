//! imports/exports for WebAssembly actors
#![cfg(target_arch = "wasm32")]

use crate::{
    common::Message,
    error::{RpcError, RpcResult},
};
use async_trait::async_trait;

#[link(wasm_import_module = "wasmbus")]
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
pub fn host_call(binding: &str, ns: &str, op: &str, msg: &[u8]) -> RpcResult<Vec<u8>> {
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
pub struct WasmHost {
    pub target: crate::core::WasmCloudEntity,
}

impl WasmHost {
    /// constructs a Transport for sending messages to a capability provider
    pub fn to_provider<T1: ToString, T2: ToString>(
        contract_id: T1,
        link_name: T2,
    ) -> RpcResult<Self> {
        Ok(WasmHost {
            target: crate::core::WasmCloudEntity::new_provider(contract_id, link_name)?,
        })
    }

    /// constructs a WasmHost Transport for sending messages to another actor
    pub fn to_actor<T: ToString>(id: T) -> RpcResult<Self> {
        Ok(WasmHost {
            target: crate::core::WasmCloudEntity::new_actor(id.to_string())?,
        })
    }
}

#[async_trait]
impl crate::common::Transport for WasmHost {
    async fn send(
        &self,
        _ctx: &crate::common::Context,
        req: Message<'_>,
        _opts: Option<crate::common::SendOpts>,
    ) -> Result<Vec<u8>, RpcError> {
        let res = if !self.target.public_key.is_empty() {
            // actor-to-actor calls use namespace for the actor target identifier
            host_call("", &self.target.public_key, req.method, req.arg.as_ref())?
        } else {
            host_call(
                &self.target.link_name,
                &self.target.contract_id,
                req.method,
                req.arg.as_ref(),
            )?
        };
        Ok(res)
    }

    fn set_timeout(&self, _interval: std::time::Duration) {
        // not applicable here - host sets timeout, so ignore this
    }
}

//#[cold]
#[inline(never)]
pub fn console_log(s: &str) {
    unsafe {
        __console_log(s.as_ptr(), s.len());
    }
}
