/// Invoke an operation on the host
pub fn call(
    binding: &str,
    namespace: &str,
    operation: &str,
    payload: Option<&[u8]>,
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

    let (pld_ptr, pld_len) = payload
        .map(|payload| (payload.as_ptr(), payload.len()))
        .unwrap_or(([].as_ptr(), 0));
    match unsafe {
        __host_call(
            binding.as_ptr(),
            binding.len(),
            namespace.as_ptr(),
            namespace.len(),
            operation.as_ptr(),
            operation.len(),
            pld_ptr,
            pld_len,
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
