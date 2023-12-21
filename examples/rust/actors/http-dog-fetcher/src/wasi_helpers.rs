//! This module contains helper functions for interacting with WASI interfaces

pub(crate) mod io {
    pub(crate) const STREAM_READ_BUFFER_SIZE: u64 = 4096;

    use crate::wasi::http::types::*;
    use wasmcloud_actor::info;
    /// Helper function to read from an input stream and write to an output stream
    pub(crate) fn connect_streams(input: InputStream, output: OutputStream) {
        while let Ok(buf) = input.read(STREAM_READ_BUFFER_SIZE) {
            if buf.is_empty() {
                break;
            }
            let amt = output.check_write().expect("failed to check write");
            info!("can write {} will write {}", amt, buf.len());
            if amt < STREAM_READ_BUFFER_SIZE {
                output
                    .blocking_flush()
                    .expect("failed to flush output stream");
            }
            output
                .write(&buf)
                .expect("failed to write to output stream");
        }
        output
            .blocking_flush()
            .expect("failed to flush output stream");
    }
}

pub(crate) mod http {
    use crate::wasi::{self, http::types::*};
    use wasmcloud_actor::error;
    /// Helper function to construct and send an outgoing HTTP request.
    pub(crate) fn outgoing_http_request(
        scheme: Scheme,
        authority: &str,
        path: &str,
    ) -> Result<IncomingResponse, ErrorCode> {
        let request = OutgoingRequest::new(Fields::new());
        request
            .set_authority(Some(authority))
            .expect("failed to set authority");
        request
            .set_path_with_query(Some(path))
            .expect("failed to set path with query");
        request
            .set_scheme(Some(&scheme))
            .expect("failed to set scheme");
        let request_body = request.body().expect("request body not found");
        OutgoingBody::finish(request_body, None).expect("failed to finish sending request body");

        let future_response = wasi::http::outgoing_handler::handle(request, None)?;
        future_response.subscribe().block();
        if let Some(Ok(incoming_response)) = future_response.get() {
            incoming_response
        } else {
            error!("failed to get http response");
            Err(ErrorCode::ConfigurationError)
        }
    }
}
