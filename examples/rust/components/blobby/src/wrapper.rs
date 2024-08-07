// This is adapted from `wasmcloud-component` but instead is impl'd directly on the type. This is
// something we should probably make part of wit-bindgen or something else since this will be a
// common thing people will need in Rust

use crate::wasi::{
    io::streams::{InputStream, OutputStream, StreamError},
    logging::logging::{log, Level},
};

/// Helper function to read all the data from the input stream and write it to the output stream
/// Optionally takes a length to limit the number of bytes read from the input stream
pub fn stream_input_to_output(
    data: InputStream,
    out: OutputStream,
    len: Option<u64>,
) -> Result<(), StreamError> {
    let mut total_len = len.unwrap_or(u64::MAX);
    loop {
        match out.blocking_splice(&data, total_len) {
            Ok(bytes_spliced) => match total_len.checked_sub(bytes_spliced) {
                Some(0) | None => return Ok(()),
                Some(new_len) => total_len = new_len,
            },
            Err(e) => match e {
                StreamError::Closed => {
                    return Ok(());
                }
                StreamError::LastOperationFailed(e) => {
                    log(
                        Level::Error,
                        "stream_input_to_output",
                        format!("last operation failed: {:?}", e).as_str(),
                    );
                    return Err(StreamError::LastOperationFailed(e));
                }
            },
        }
    }
}
