//! Keeping the host's signing values out of a response body.
//!
//! S3 echoes what it was sent in its 400 and 403 errors: the access key ID,
//! the session token, and the whole canonical request, in hex as well as
//! plain. Scrubbing every spelling of that is not practical, so those bodies
//! are replaced with a few fields known not to carry it. Any other body has
//! each value masked as it streams, with a mask of the same length so the
//! framing the server sent still holds. That masking also alters an object
//! that legitimately contains the access key ID or session token.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::{Body, Frame};
use zeroize::{Zeroize as _, Zeroizing};

use crate::host::egress_credential::Substituted;

/// The most of an error body read to find the fields kept from it.
const ERROR_BODY_LIMIT: usize = 64 * 1024;

/// The message a replaced error body carries in place of the server's.
const REPLACED: &str = "The host signed this request; the server's details were removed \
                        because they can echo its credentials.";

/// Whether a byte may stand in a kept field.
type Allowed = fn(u8) -> bool;

/// The fields kept from a replaced error, and the bytes each may hold.
const KEPT: [(&str, Allowed); 3] = [
    ("Code", |b| b.is_ascii_alphanumeric() || b == b'.'),
    ("Region", |b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
    }),
    ("BucketName", |b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'
    }),
];

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
    if !done.scrubs_body() {
        return response.map(BodyExt::boxed_unsync);
    }
    if matches!(
        response.status(),
        http::StatusCode::BAD_REQUEST | http::StatusCode::FORBIDDEN
    ) {
        let headers = response.headers_mut();
        headers.remove(http::header::CONTENT_LENGTH);
        headers.remove(http::header::CONTENT_ENCODING);
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/xml"),
        );
        return response.map(|body| {
            let replaced = futures::stream::once(replace_error(body, done));
            http_body_util::StreamBody::new(replaced).boxed_unsync()
        });
    }
    response.map(|body| ScrubBody::new(body, done).boxed_unsync())
}

/// The error body S3 would have sent, with only the [`KEPT`] fields.
async fn replace_error<B>(mut body: B, done: Arc<Substituted>) -> Result<Frame<Bytes>, B::Error>
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
    let mut kept = String::new();
    for (name, allowed) in KEPT {
        if let Some(value) = field(&read, name, allowed)
            && !done
                .signed()
                .iter()
                .any(|(sent, _)| memchr::memmem::find(value.as_bytes(), sent).is_some())
        {
            kept.push_str(&format!("<{name}>{value}</{name}>"));
        }
        if name == "Code" {
            kept.push_str(&format!("<Message>{REPLACED}</Message>"));
        }
    }
    Ok(Frame::data(Bytes::from(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error>{kept}</Error>"
    ))))
}

/// The text of an S3 XML error's first `<name>`, if every byte is `allowed`.
fn field<'a>(body: &'a [u8], name: &str, allowed: Allowed) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = memchr::memmem::find(body, open.as_bytes())? + open.len();
    let rest = body.get(start..)?;
    let end = memchr::memmem::find(rest, close.as_bytes())?;
    let value = rest.get(..end)?;
    (!value.is_empty() && value.len() <= 64 && value.iter().all(|&b| allowed(b)))
        .then(|| std::str::from_utf8(value).ok())
        .flatten()
}

/// A body with each recorded value masked as it streams.
struct ScrubBody<B> {
    inner: B,
    done: Arc<Substituted>,
    longest: usize,
    /// The tail of what has been read that begins some value.
    carry: Zeroizing<Vec<u8>>,
    queued: VecDeque<Frame<Bytes>>,
    finished: bool,
}

impl<B> ScrubBody<B> {
    fn new(inner: B, done: Arc<Substituted>) -> Self {
        let longest = done
            .signed()
            .iter()
            .map(|(v, _)| v.len())
            .max()
            .unwrap_or(1);
        Self {
            inner,
            done,
            longest,
            carry: Zeroizing::new(Vec::new()),
            queued: VecDeque::new(),
            finished: false,
        }
    }

    fn emit(&mut self, data: Bytes) {
        if !data.is_empty() {
            self.queued.push_back(Frame::data(data));
        }
    }

    fn mask(&mut self, len: usize) {
        self.emit(Bytes::from(vec![b'*'; len]));
    }

    /// The leftmost recorded value in `text`, as its offset and length,
    /// preferring the longest of those that start there.
    fn leftmost(&self, text: &[u8]) -> Option<(usize, usize)> {
        self.done
            .signed()
            .iter()
            .filter_map(|(value, _)| memchr::memmem::find(text, value).map(|at| (at, value.len())))
            .min_by_key(|&(at, len)| (at, std::cmp::Reverse(len)))
    }

    /// The length of the longest suffix of `text` that is a proper prefix of
    /// some recorded value.
    fn partial(&self, text: &[u8]) -> usize {
        let mut held = 0;
        for (value, _) in self.done.signed() {
            let Some(&first) = value.first() else {
                continue;
            };
            let from = text.len().saturating_sub(value.len().saturating_sub(1));
            let tail = text.get(from..).unwrap_or_default();
            let starts = memchr::memchr_iter(first, tail).map(|at| from + at);
            if let Some(at) = starts
                .filter(|&at| at + held < text.len())
                .find(|&at| value.starts_with(text.get(at..).unwrap_or_default()))
            {
                held = held.max(text.len() - at);
            }
        }
        held
    }

    /// Queue `data` with every recorded value masked, keeping back any tail
    /// that could still begin one unless `last`.
    fn scrub(&mut self, mut data: Bytes, last: bool) {
        if !self.carry.is_empty() {
            let need = self.longest - 1;
            if data.len() < need && !last {
                let mut joined = std::mem::take(&mut *self.carry);
                joined.extend_from_slice(&data);
                data = Bytes::from(joined);
            } else {
                let carried = self.carry.len();
                let mut window = Zeroizing::new(Vec::with_capacity(carried + need));
                window.extend_from_slice(&self.carry);
                window.extend_from_slice(data.get(..need.min(data.len())).unwrap_or_default());
                self.carry.zeroize();
                match self.leftmost(&window) {
                    Some((at, len)) if at < carried => {
                        self.emit(Bytes::copy_from_slice(window.get(..at).unwrap_or_default()));
                        self.mask(len);
                        data = data.slice(at + len - carried..);
                    }
                    _ => self.emit(Bytes::copy_from_slice(
                        window.get(..carried).unwrap_or_default(),
                    )),
                }
            }
        }
        let mut at = 0;
        while let Some((found, len)) = self.leftmost(data.get(at..).unwrap_or_default()) {
            self.emit(data.slice(at..at + found));
            self.mask(len);
            at += found + len;
        }
        let held = if last {
            0
        } else {
            self.partial(data.get(at..).unwrap_or_default())
        };
        let end = data.len() - held;
        self.emit(data.slice(at..end));
        self.carry = Zeroizing::new(data.get(end..).unwrap_or_default().to_vec());
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
        loop {
            if let Some(frame) = this.queued.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if this.finished {
                return Poll::Ready(None);
            }
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => this.scrub(data, false),
                    Err(trailers) => {
                        this.finished = true;
                        this.scrub(Bytes::new(), true);
                        this.queued.push_back(trailers);
                    }
                },
                Some(Err(err)) => return Poll::Ready(Some(Err(err))),
                None => {
                    this.finished = true;
                    this.scrub(Bytes::new(), true);
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

    async fn error(status: u16, body: &'static [u8], done: Arc<Substituted>) -> (bool, String) {
        let mut response = hyper::Response::new(chunked([body]));
        *response.status_mut() = http::StatusCode::from_u16(status).unwrap();
        response.headers_mut().insert(
            http::header::CONTENT_LENGTH,
            http::HeaderValue::from(body.len()),
        );
        let scrubbed = scrub_signed_response(response, done);
        let framed = scrubbed
            .headers()
            .contains_key(http::header::CONTENT_LENGTH);
        (framed, String::from_utf8(read(scrubbed).await).unwrap())
    }

    #[tokio::test]
    async fn a_value_split_across_chunks_is_masked_at_its_length() {
        let response = hyper::Response::new(chunked([
            b"key=ASIA",
            b"REAL1234 and to",
            b"ken TOKEN!",
            b"ASIAREAL12",
            b"34",
        ]));
        let body = read(scrub_signed_response(
            response,
            done(&[b"ASIAREAL1234", b"TOKEN"]),
        ))
        .await;
        assert_eq!(body, b"key=************ and token *****!************");
    }

    /// Only a tail that could begin a value waits for the next frame;
    /// anything else goes out as it arrived, without a copy.
    #[tokio::test]
    async fn a_frame_with_no_partial_match_passes_straight_through() {
        static FIRST: &[u8] = b"hello";
        let mut body = scrub_signed_response(
            hyper::Response::new(chunked([FIRST, b"abcASI", b"AREAL1234"])),
            done(&[b"ASIAREAL1234"]),
        )
        .into_body();
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(first.as_ptr(), FIRST.as_ptr(), "neither held nor copied");
        let second = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(second, "abc");
        let rest = body.collect().await.unwrap().to_bytes();
        assert_eq!(rest, "************");
    }

    #[tokio::test]
    async fn a_short_frame_after_a_partial_match_still_masks_it() {
        let response = hyper::Response::new(chunked([b"xxASIA", b"RE", b"AL1234yy"]));
        let body = read(scrub_signed_response(response, done(&[b"ASIAREAL1234"]))).await;
        assert_eq!(body, b"xx************yy");
    }

    #[tokio::test]
    async fn a_partial_match_that_does_not_complete_is_sent_unchanged() {
        let response = hyper::Response::new(chunked([b"xxASIAREAL", b"9999 more"]));
        let body = read(scrub_signed_response(response, done(&[b"ASIAREAL1234"]))).await;
        assert_eq!(body, b"xxASIAREAL9999 more");
    }

    #[tokio::test]
    async fn a_client_error_body_keeps_only_its_allowed_fields() {
        let (framed, body) = error(
            403,
            b"<Error><Code>SignatureDoesNotMatch</Code><AWSAccessKeyId>ASIAREAL1234</AWSAccessKeyId>\
              <Region>us-east-1</Region><BucketName>reports.example</BucketName>\
              <CanonicalRequestBytes>41 53 49 41</CanonicalRequestBytes></Error>",
            done(&[b"ASIAREAL1234"]),
        )
        .await;
        assert!(!framed);
        assert!(
            body.contains("<Code>SignatureDoesNotMatch</Code>")
                && body.contains("<Region>us-east-1</Region>")
                && body.contains("<BucketName>reports.example</BucketName>"),
            "{body}"
        );
        assert!(!body.contains("ASIA") && !body.contains("41 53"), "{body}");
    }

    #[tokio::test]
    async fn a_kept_field_holding_a_value_is_dropped() {
        let (_, body) = error(
            400,
            b"<Error><Code>InvalidBucketName</Code><BucketName>asiareal1234</BucketName></Error>",
            done(&[b"asiareal1234"]),
        )
        .await;
        assert!(body.contains("<Code>InvalidBucketName</Code>"), "{body}");
        assert!(!body.contains("asiareal1234"), "{body}");
    }

    #[tokio::test]
    async fn other_errors_are_masked_not_replaced() {
        let (framed, body) = error(
            404,
            b"<Error><Code>NoSuchKey</Code><Key>ASIAREAL1234</Key></Error>",
            done(&[b"ASIAREAL1234"]),
        )
        .await;
        assert!(framed, "masking keeps the length");
        assert_eq!(
            body,
            "<Error><Code>NoSuchKey</Code><Key>************</Key></Error>"
        );
    }

    #[tokio::test]
    async fn nothing_signed_or_a_kept_body_is_left_alone() {
        let response = hyper::Response::new(chunked([b"ASIAREAL1234"]));
        let body = read(scrub_signed_response(response, done(&[]))).await;
        assert_eq!(body, b"ASIAREAL1234");

        let mut kept = Substituted::default();
        kept.note_signed(&Arc::from("wash_ph_7Qm2xYk9Lr4Tz8Vb1Nc6"), b"ASIAREAL1234");
        kept.keep_body();
        let (framed, body) = error(403, b"ASIAREAL1234 from a Lambda", Arc::new(kept)).await;
        assert!(framed);
        assert_eq!(body, "ASIAREAL1234 from a Lambda");
    }
}
