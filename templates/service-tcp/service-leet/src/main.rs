use anyhow::Result;
use wstd::io::{AsyncRead, AsyncWrite};
use wstd::iter::AsyncIterator;
use wstd::net::TcpListener;

#[wstd::main]
async fn main() -> Result<()> {
    let listener = TcpListener::bind("0.0.0.0:7777").await?;
    let mut incoming = listener.incoming();

    while let Some(stream) = incoming.next().await {
        let stream = stream?;
        println!("Accepted from: {}", stream.peer_addr()?);
        wstd::runtime::spawn(async move {
            let mut stream = stream;
            let mut buf = [0u8; 1024];
            let mut line_buf = Vec::new();

            loop {
                let n = stream.read(&mut buf).await?;
                if n == 0 {
                    break;
                }

                for &byte in &buf[..n] {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        let response = to_leet_speak(&line);
                        stream.write_all(response.as_bytes()).await?;
                        stream.write_all(b"\n").await?;
                        stream.flush().await?;
                        line_buf.clear();
                    } else {
                        line_buf.push(byte);
                    }
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    Ok(())
}

fn to_leet_speak(input: &str) -> String {
    input
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            'a' => '4',
            'e' => '3',
            'i' => '1',
            'o' => '0',
            's' => '5',
            't' => '7',
            'l' => '1',
            _ => c,
        })
        .collect()
}
