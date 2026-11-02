use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Complete one handshake advertising `advertised` and keep the connection open.
pub(crate) async fn accept_advertising(
    listener: &TcpListener,
    advertised: SocketAddr,
) -> anyhow::Result<TcpStream> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut socket, _) = listener.accept().await?;
        socket
            .write_all(
                format!("INFO {{\"max_payload\":1048576,\"connect_urls\":[\"{advertised}\"]}}\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut reader = BufReader::new(&mut socket);
        loop {
            let mut line = String::new();
            anyhow::ensure!(reader.read_line(&mut line).await? > 0, "closed before PING");
            if line.starts_with("PING") {
                break;
            }
        }
        socket.write_all(b"PONG\r\n").await?;
        Ok(socket)
    })
    .await?
}
