/// Export libFuzzer's byte-slice callback. Panics abort at the C boundary.
#[macro_export]
macro_rules! fuzz_target {
    (|$data:ident: &str| $body:block) => {
        $crate::fuzz_target!(|bytes: &[u8]| {
            if let Ok($data) = std::str::from_utf8(bytes) {
                let _: () = (|$data: &str| $body)($data);
            }
        });
    };
    (|$data:ident: &[u8]| $body:block) => {
        /// # Safety
        /// `data` must point to `size` readable bytes when `size` is nonzero.
        // SAFETY: each fuzz binary exports one libFuzzer callback.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
            let $data: &[u8] = if size == 0 {
                &[]
            } else {
                // SAFETY: libFuzzer keeps the input readable until the callback returns.
                unsafe { std::slice::from_raw_parts(data, size) }
            };
            let _: () = (|$data: &[u8]| $body)($data);
            0
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    fuzz_target!(|data: &str| {
        assert!(data.is_empty() || data == "seed");
        CALLS.fetch_add(1, Ordering::Relaxed);
    });

    #[test]
    fn dispatches_empty_and_utf8_inputs_and_skips_invalid_utf8() {
        let seed = b"seed";
        let invalid = [0xff];
        // SAFETY: each nonempty input stays alive until its callback returns.
        unsafe {
            assert_eq!(LLVMFuzzerTestOneInput(std::ptr::null(), 0), 0);
            assert_eq!(LLVMFuzzerTestOneInput(seed.as_ptr(), seed.len()), 0);
            assert_eq!(LLVMFuzzerTestOneInput(invalid.as_ptr(), invalid.len()), 0);
        }
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
    }
}
