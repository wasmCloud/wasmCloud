//! A WASIp3 command that reports its invocation and ends the way `EXIT_MODE`
//! says: `return-ok`/`return-err` return from `run`, `exit-ok`/`exit-err` call
//! `wasi:cli/exit` first and would otherwise return the opposite status.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest;
use bindings::wasi::cli::{environment, exit, stderr, stdin, stdout, types::ErrorCode};
use wit_bindgen::{FutureReader, StreamReader, StreamResult};

struct Component;

async fn write(
    sink: fn(StreamReader<u8>) -> FutureReader<Result<(), ErrorCode>>,
    bytes: Vec<u8>,
) -> Result<(), ()> {
    let (mut tx, rx) = bindings::wit_stream::new();
    wit_bindgen::spawn_local(async move {
        tx.write_all(bytes).await;
        drop(tx);
    });
    sink(rx).await.map_err(|_| ())
}

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        let args = environment::get_arguments();
        let mut env = environment::get_environment();
        env.sort();
        let (mut input, _closed) = stdin::read_via_stream();
        let mut received = Vec::new();
        loop {
            let (result, chunk) = input.read(Vec::with_capacity(1024)).await;
            received.extend(chunk);
            if matches!(result, StreamResult::Dropped) {
                break;
            }
        }

        let mut report = format!("args={}\n", args.join(" ")).into_bytes();
        let env = env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        report.extend(format!("env={env}\n").into_bytes());
        report.extend(b"stdin=");
        report.extend(received);
        report.push(b'\n');
        write(stdout::write_via_stream, report).await?;
        write(stderr::write_via_stream, b"diagnostic".to_vec()).await?;

        let mode = environment::get_environment()
            .into_iter()
            .find(|(key, _)| key == "EXIT_MODE")
            .map(|(_, value)| value)
            .unwrap_or_default();
        match mode.as_str() {
            "return-err" => Err(()),
            "exit-ok" => {
                exit::exit(Ok(()));
                Err(())
            }
            "exit-err" => {
                exit::exit(Err(()));
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

bindings::export!(Component with_types_in bindings);
