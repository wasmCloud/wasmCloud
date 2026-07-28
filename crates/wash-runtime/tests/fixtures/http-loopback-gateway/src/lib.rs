//! HTTP -> loopback gateway: each request opens a virtualized-loopback TCP
//! connection to the `svc-tcp-echo` service (127.0.0.1:9099), sends `ping`,
//! and returns the echoed reply as the response body. The pair reproduces a
//! long-lived service reached guest-to-guest, with the HTTP side driving one
//! request at a time.

mod bindings {
    wit_bindgen::generate!({ world: "http-loopback-gateway", generate_all });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};

const ECHO_PORT: u16 = 9099;

struct Component;

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let reply = echo_roundtrip()
            .await
            .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))?;
        Ok(make_response(reply))
    }
}

async fn echo_roundtrip() -> Result<Vec<u8>, &'static str> {
    let client = TcpSocket::create(IpAddressFamily::Ipv4).map_err(|_| "create failed")?;
    client
        .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: ECHO_PORT,
            address: (127, 0, 0, 1),
        }))
        .await
        .map_err(|_| "connect failed")?;

    // Send the request; dropping the writer ends the stream, which the echo
    // service observes as end-of-request.
    let (mut tx, tx_stream) = bindings::wit_stream::new();
    let (send_result, ()) = futures::join!(async { client.send(tx_stream).await }, async {
        tx.write_all(b"ping".to_vec()).await;
        drop(tx);
    });
    send_result.map_err(|_| "send failed")?;

    let (mut rx, _rx_result) = client.receive();
    let (_result, reply) = rx.read(Vec::with_capacity(1024)).await;
    if reply.is_empty() {
        return Err("empty reply");
    }
    Ok(reply)
}

fn make_response(body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    response
}

bindings::export!(Component with_types_in bindings);
