mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest;
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket, UdpSocket,
};
use wit_bindgen::StreamResult;

struct Component;

async fn test_tcp_loopback() {
    let listener = TcpSocket::create(IpAddressFamily::Ipv4).unwrap();
    listener
        .bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: 0,
            address: (127, 0, 0, 1),
        }))
        .unwrap();
    listener.set_listen_backlog_size(2).unwrap();
    let mut accept = listener.listen().unwrap();

    let addr = listener.get_local_address().unwrap();

    let message = b"hello tcp p3";

    futures::join!(
        async {
            // Client side: connect and send data
            let client = TcpSocket::create(IpAddressFamily::Ipv4).unwrap();
            client.connect(addr).await.unwrap();
            let (mut data_tx, data_rx) = bindings::wit_stream::new();
            futures::join!(
                async {
                    client.send(data_rx).await.unwrap();
                },
                async {
                    data_tx.write_all(message.to_vec()).await;
                    drop(data_tx);
                }
            );
        },
        async {
            // Server side: accept and receive data
            let sock = accept.next().await.unwrap();
            let (mut data_rx, fut) = sock.receive();
            let (result, data) = data_rx.read(Vec::with_capacity(100)).await;
            assert_eq!(result, StreamResult::Complete(message.len()));
            assert_eq!(&data, message);

            // Wait for stream end
            let (result, _) = data_rx.read(Vec::with_capacity(1)).await;
            assert_eq!(result, StreamResult::Dropped);

            fut.await.unwrap();
        }
    );
}

async fn test_udp_loopback() {
    let server = UdpSocket::create(IpAddressFamily::Ipv4).unwrap();
    server
        .bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: 0,
            address: (127, 0, 0, 1),
        }))
        .unwrap();

    let server_addr = server.get_local_address().unwrap();

    let client = UdpSocket::create(IpAddressFamily::Ipv4).unwrap();
    client
        .bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: 0,
            address: (127, 0, 0, 1),
        }))
        .unwrap();

    let message = b"hello udp p3";
    client
        .send(message.to_vec(), Some(server_addr))
        .await
        .unwrap();

    let (data, _remote_addr) = server.receive().await.unwrap();
    assert_eq!(data, message);
}

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        test_tcp_loopback().await;
        test_udp_loopback().await;
        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);
