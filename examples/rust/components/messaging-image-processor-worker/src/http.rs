use anyhow::{anyhow, bail, ensure, Context as _, Result};
use bytes::{Bytes, BytesMut};

use crate::bindings::wasi::http::outgoing_handler;
use crate::bindings::wasi::http::types::{IncomingBody, OutgoingRequest};
use crate::bindings::wasi::io::streams::StreamError;

use crate::MAX_READ_BYTES;

impl OutgoingRequest {
    pub(crate) fn fetch_bytes(self) -> Result<Option<Bytes>> {
        let resp =
            outgoing_handler::handle(self, None).map_err(|e| anyhow!("request failed: {e}"))?;
        resp.subscribe().block();
        let response = resp
            .get()
            .context("HTTP request response missing")?
            .map_err(|()| anyhow!("HTTP request response requested more than once"))?
            .map_err(|code| anyhow!("HTTP request failed (error code {code})"))?;

        if response.status() != 200 {
            bail!("response failed, status code [{}]", response.status());
        }

        let response_body = response
            .consume()
            .map_err(|()| anyhow!("failed to get incoming request body"))?;

        let mut buf = BytesMut::with_capacity(MAX_READ_BYTES as usize);
        let stream = response_body
            .stream()
            .expect("failed to get HTTP request response stream");
        loop {
            match stream.read(MAX_READ_BYTES as u64) {
                Ok(bytes) if bytes.is_empty() => break,
                Ok(bytes) => {
                    ensure!(
                        bytes.len() <= MAX_READ_BYTES as usize,
                        "read more bytes than requested"
                    );
                    buf.extend(bytes);
                }
                Err(StreamError::Closed) => break,
                Err(e) => bail!("failed to read bytes: {e}"),
            }
        }
        let _ = IncomingBody::finish(response_body);

        Ok(Some(buf.freeze()))
    }
}
