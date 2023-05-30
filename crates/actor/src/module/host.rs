/// Invoke an operation on the host
pub fn call(bd: &str, ns: &str, op: &str, pld: Option<&[u8]>) -> Result<Vec<u8>, String> {
    #[link(wasm_import_module = "wasmbus")]
    extern "C" {
        pub fn __host_call(
            bd_ptr: *const u8,
            bd_len: usize,
            ns_ptr: *const u8,
            ns_len: usize,
            op_ptr: *const u8,
            op_len: usize,
            pld_ptr: *const u8,
            pld_len: usize,
        ) -> usize;

        pub fn __host_response(ptr: *mut u8);
        pub fn __host_response_len() -> usize;

        pub fn __host_error(ptr: *mut u8);
        pub fn __host_error_len() -> usize;
    }

    let (pld_ptr, pld_len) = pld
        .map(|pld| (pld.as_ptr(), pld.len()))
        .unwrap_or(([].as_ptr(), 0));
    match unsafe {
        __host_call(
            bd.as_ptr(),
            bd.len(),
            ns.as_ptr(),
            ns.len(),
            op.as_ptr(),
            op.len(),
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
