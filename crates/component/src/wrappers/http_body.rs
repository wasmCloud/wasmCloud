use bytes::Buf;
use futures::StreamExt;

const CHUNK_SIZE: u64 = 1024 * 1024;

impl http_body::Body for crate::http::IncomingBody {
    type Data = bytes::Bytes;

    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        std::task::Poll::Ready(match self.stream.read(CHUNK_SIZE) {
            Ok(d) => Some(Ok(http_body::Frame::data(d.into()))),
            Err(_e) => todo!(),
        })
    }
}

pub async fn write_axum_response_to_wasi<T: http_body::Body>(
    response: http::Response<T>,
    outparam: wasi::http::types::ResponseOutparam,
) {
    let (
        http::response::Parts {
            status, headers, ..
        },
        body,
    ) = response.into_parts();
    let headers = crate::TryInto::try_into(headers).unwrap();
    let resp_tx = wasi::http::types::OutgoingResponse::new(headers);
    if let Err(()) = resp_tx.set_status_code(status.as_u16()) {
        todo!("failed to set status code");
    }

    let Ok(resp_body) = resp_tx.body() else {
        todo!("failed to get body");
    };

    wasi::http::types::ResponseOutparam::set(outparam, Ok(resp_tx));

    let out = resp_body.write().expect("outgoing stream");
    let body_stream = http_body_util::BodyDataStream::new(body);
    body_stream
        .for_each(|chunk| {
            match chunk {
                Ok(chunk) => {
                    let chunk = chunk.chunk();
                    out.write(chunk).expect("writing response");
                }
                Err(_) => todo!(),
            };
            std::future::ready(())
        })
        .await;

    out.blocking_flush().unwrap();
    drop(out);
    wasi::http::types::OutgoingBody::finish(resp_body, None).unwrap();
}
