//! A WASIp3 command that reads `FILE` from its first preopen through
//! `read-via-stream`, offering `CHUNK` bytes per stream read, and prints
//! `bytes=<total> reads=<count> largest=<bytes>`.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest;
use bindings::wasi::cli::{environment, stdout};
use bindings::wasi::filesystem::{preopens, types};
use wit_bindgen::StreamResult;

struct Component;

fn env(key: &str) -> Option<String> {
    environment::get_environment()
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        let name = env("FILE").ok_or(())?;
        let chunk: usize = env("CHUNK").and_then(|v| v.parse().ok()).ok_or(())?;
        let (dir, _) = preopens::get_directories().into_iter().next().ok_or(())?;
        let file = dir
            .open_at(
                types::PathFlags::empty(),
                name,
                types::OpenFlags::empty(),
                types::DescriptorFlags::READ,
            )
            .await
            .map_err(|_| ())?;
        let (mut rx, done) = file.read_via_stream(0);
        let mut buf = Vec::with_capacity(chunk);
        let (mut bytes, mut reads, mut largest) = (0, 0, 0);
        loop {
            let (result, mut got) = rx.read(buf).await;
            reads += 1;
            bytes += got.len();
            largest = largest.max(got.len());
            got.clear();
            buf = got;
            if matches!(result, StreamResult::Dropped) {
                break;
            }
        }
        done.await.map_err(|_| ())?;

        let report = format!("bytes={bytes} reads={reads} largest={largest}").into_bytes();
        let (mut tx, out) = bindings::wit_stream::new();
        wit_bindgen::spawn_local(async move {
            tx.write_all(report).await;
            drop(tx);
        });
        stdout::write_via_stream(out).await.map_err(|_| ())
    }
}

bindings::export!(Component with_types_in bindings);
