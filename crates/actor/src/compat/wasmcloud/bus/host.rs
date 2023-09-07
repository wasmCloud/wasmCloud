use crate::wasmcloud::bus::lattice::{ActorIdentifier, TargetEntity};

/// Invoke an operation on the host
fn host_call(
    binding: &str,
    namespace: &str,
    operation: &str,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    #[link(wasm_import_module = "wasmbus")]
    extern "C" {
        pub fn __host_call(
            binding_ptr: *const u8,
            binding_len: usize,
            namespace_ptr: *const u8,
            namespace_len: usize,
            operation_ptr: *const u8,
            operation_len: usize,
            payload_ptr: *const u8,
            payload_len: usize,
        ) -> usize;

        pub fn __host_response(ptr: *mut u8);
        pub fn __host_response_len() -> usize;

        pub fn __host_error(ptr: *mut u8);
        pub fn __host_error_len() -> usize;
    }

    match unsafe {
        __host_call(
            binding.as_ptr(),
            binding.len(),
            namespace.as_ptr(),
            namespace.len(),
            operation.as_ptr(),
            operation.len(),
            payload.as_ptr(),
            payload.len(),
        )
    } {
        1 => {
            // call succeeded
            let mut buf = vec![0; unsafe { __host_response_len() }];
            unsafe { __host_response(buf.as_mut_ptr()) };
            Ok(buf)
        }
        _ => {
            // call failed
            let mut buf = vec![0; unsafe { __host_error_len() }];
            unsafe { __host_error(buf.as_mut_ptr()) };
            match String::from_utf8(buf) {
                Ok(e) => Err(e),
                Err(e) => Err(format!(
                    "the host provided an error, which is not valid UTF-8: {e}"
                )),
            }
        }
    }
}

pub fn call_sync(
    target: Option<&TargetEntity>,
    operation: &str,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    match target {
        None => {
            let (namespace, operation) = operation
                .rsplit_once('/')
                .ok_or_else(|| "invalid operation format".to_string())?;
            host_call("", namespace, operation, payload)
        }
        Some(TargetEntity::Link(binding)) => {
            let (namespace, operation) = operation
                .rsplit_once('/')
                .ok_or_else(|| "invalid operation format".to_string())?;
            host_call(
                binding.as_deref().unwrap_or_default(),
                namespace,
                operation,
                payload,
            )
        }
        Some(TargetEntity::Actor(
            ActorIdentifier::PublicKey(namespace) | ActorIdentifier::Alias(namespace),
        )) => host_call("", namespace, operation, payload),
    }
}
