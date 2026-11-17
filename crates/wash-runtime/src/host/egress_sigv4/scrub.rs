//! Keeping the host's signing values out of a response body.
//!
//! S3 echoes what it was sent in its errors: the access key ID, the session
//! token, and the whole canonical request, in hex as well as plain. Scrubbing
//! every spelling of that is not practical, so a 4xx body is replaced with
//! its error code alone. Any other body has each value masked as it streams,
//! with a mask of the same length so the framing the server sent still holds.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::{Body, Frame};
use zeroize::Zeroizing;

use crate::host::egress_credential::Substituted;

/// The most of a 4xx body read to find its error code.
const ERROR_BODY_LIMIT: usize = 64 * 1024;

/// The message a replaced error body carries in place of the server's.
const REPLACED: &str = "The host signed this request; the server's details were removed \
                        because they can echo its credentials.";

/// `response` to a request the host re-signed, with the values recorded in
/// `done` kept out of its body.
pub(crate) fn scrub_signed_response<B>(
    mut response: hyper::Response<B>,
    done: Arc<Substituted>,
) -> hyper::Response<UnsyncBoxBody<Bytes, B::Error>>
where
    B: Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: Send + 'static,
{
    if done.signed().is_empty() {
        return response.map(BodyExt::boxed_unsync);
    }
    if response.status().is_client_error() {
        let headers = response.headers_mut();
        headers.remove(http::header::CONTENT_LENGTH);
        headers.remove(http::header::CONTENT_ENCODING);
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/xml"),
        );
        return response.map(|body| {
            let replaced = futures::stream::once(replace_error(body));
            http_body_util::StreamBody::new(replaced).boxed_unsync()
        });
    }
    response.map(|body| {
        ScrubBody {
            inner: body,
            done,
            carry: Zeroizing::new(Vec::new()),
            pending: None,
            finished: false,
        }
        .boxed_unsync()
    })
}

/// The error body S3 would have sent, with only its `<Code>` kept.
async fn replace_error<B>(mut body: B) -> Result<Frame<Bytes>, B::Error>
where
    B: Body<Data = Bytes> + Unpin,
{
    let mut read = Zeroizing::new(Vec::new());
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame?.into_data()
            && read.len() < ERROR_BODY_LIMIT
        {
            read.extend_from_slice(&data);
        }
    }
    if read.is_empty() {
        return Ok(Frame::data(Bytes::new()));
    }
    let code = error_code(&read)
        .map(|code| format!("<Code>{code}</Code>"))
        .unwrap_or_default();
    Ok(Frame::data(Bytes::from(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error>{code}<Message>{REPLACED}</Message></Error>"
    ))))
}

/// The `<Code>` of an S3 XML error, if it is a plain identifier.
fn error_code(body: &[u8]) -> Option<&str> {
    let start = memchr::memmem::find(body, b"<Code>")? + "<Code>".len();
    let rest = body.get(start..)?;
    let end = memchr::memmem::find(rest, b"</Code>")?;
    let code = std::str::from_utf8(rest.get(..end)?).ok()?;
    (!code.is_empty()
        && code.len() <= 64
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'))
    .then_some(code)
}

/// A body with each recorded value masked as it streams.
struct ScrubBody<B> {
    inner: B,
    done: Arc<Substituted>,
    /// The tail of what has been read that could still begin a value.
    carry: Zeroizing<Vec<u8>>,
    /// Trailers held back while the carry is flushed ahead of them.
    pending: Option<Frame<Bytes>>,
    finished: bool,
}

impl<B> ScrubBody<B> {
    /// Mask `data` appended to the carry, keeping back any tail that could
    /// still begin a value unless `last`.
    fn scrub(&mut self, data: &[u8], last: bool) -> Bytes {
        let mut text = std::mem::take(&mut *self.carry);
        text.extend_from_slice(data);
        let text = Zeroizing::new(text);
        let mut out = Vec::with_capacity(text.len());
        let mut at = 0;
        loop {
            let rest = text.get(at..).unwrap_or_default();
            let next = self
                .done
                .signed()
                .iter()
                .filter_map(|(value, _)| {
                    memchr::memmem::find(rest, value).map(|found| (found, value.len()))
                })
                .min_by_key(|(found, _)| *found);
            let Some((found, len)) = next else { break };
            out.extend_from_slice(rest.get(..found).unwrap_or_default());
            out.resize(out.len() + len, b'*');
            at += found + len;
        }
        let rest = text.get(at..).unwrap_or_default();
        let keep = if last {
            0
        } else {
            let longest = self.done.signed().iter().map(|(v, _)| v.len()).max();
            longest.unwrap_or(1).saturating_sub(1).min(rest.len())
        };
        let (emit, held) = rest.split_at(rest.len() - keep);
        out.extend_from_slice(emit);
        self.carry = Zeroizing::new(held.to_vec());
        Bytes::from(out)
    }
}

impl<B> Body for ScrubBody<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        if let Some(frame) = this.pending.take() {
            return Poll::Ready(Some(Ok(frame)));
        }
        if this.finished {
            return Poll::Ready(None);
        }
        loop {
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => {
                        let out = this.scrub(&data, false);
                        if !out.is_empty() {
                            return Poll::Ready(Some(Ok(Frame::data(out))));
                        }
                    }
                    Err(trailers) => {
                        this.finished = true;
                        let out = this.scrub(&[], true);
                        if out.is_empty() {
                            return Poll::Ready(Some(Ok(trailers)));
                        }
                        this.pending = Some(trailers);
                        return Poll::Ready(Some(Ok(Frame::data(out))));
                    }
                },
                Some(Err(err)) => return Poll::Ready(Some(Err(err))),
                None => {
                    this.finished = true;
                    let out = this.scrub(&[], true);
                    return Poll::Ready((!out.is_empty()).then(|| Ok(Frame::data(out))));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(values: &[&[u8]]) -> Arc<Substituted> {
        let mut done = Substituted::default();
        let placeholder: Arc<str> = Arc::from("wash_ph_7Qm2xYk9Lr4Tz8Vb1Nc6");
        for value in values {
            done.note_signed(&placeholder, value);
        }
        Arc::new(done)
    }

    fn chunked<const N: usize>(
        chunks: [&'static [u8]; N],
    ) -> UnsyncBoxBody<Bytes, std::convert::Infallible> {
        let frames = chunks
            .into_iter()
            .map(|chunk| Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from_static(chunk))));
        http_body_util::StreamBody::new(futures::stream::iter(frames)).boxed_unsync()
    }

    async fn read(
        response: hyper::Response<UnsyncBoxBody<Bytes, std::convert::Infallible>>,
    ) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn a_value_split_across_chunks_is_masked_at_its_length() {
        let response =
            hyper::Response::new(chunked([b"key=ASIA", b"REAL1234 and to", b"ken TOKEN!"]));
        let body = read(scrub_signed_response(
            response,
            done(&[b"ASIAREAL1234", b"TOKEN"]),
        ))
        .await;
        assert_eq!(body, b"key=************ and token *****!");
    }

    #[tokio::test]
    async fn a_client_error_body_keeps_only_its_code() {
        let mut response = hyper::Response::new(chunked([
            b"<Error><Code>SignatureDoesNotMatch</Code><AWSAccessKeyId>ASIAREAL1234",
            b"</AWSAccessKeyId><CanonicalRequestBytes>41 53 49 41</CanonicalRequestBytes></Error>",
        ]));
        *response.status_mut() = http::StatusCode::FORBIDDEN;
        response.headers_mut().insert(
            http::header::CONTENT_LENGTH,
            http::HeaderValue::from_static("150"),
        );
        let scrubbed = scrub_signed_response(response, done(&[b"ASIAREAL1234"]));
        assert!(
            !scrubbed
                .headers()
                .contains_key(http::header::CONTENT_LENGTH)
        );
        let body = String::from_utf8(read(scrubbed).await).unwrap();
        assert!(
            body.contains("<Code>SignatureDoesNotMatch</Code>"),
            "{body}"
        );
        assert!(!body.contains("ASIA") && !body.contains("41 53"), "{body}");
    }

    #[tokio::test]
    async fn nothing_signed_leaves_the_body_alone() {
        let response = hyper::Response::new(chunked([b"ASIAREAL1234"]));
        let body = read(scrub_signed_response(response, done(&[]))).await;
        assert_eq!(body, b"ASIAREAL1234");
    }
}
