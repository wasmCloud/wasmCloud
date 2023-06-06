use std::env::args;
use std::io::{stdin, stdout, Write};

use anyhow::Context;
use wasmcloud_actor::{HttpRequest, HttpResponse};

fn main() -> anyhow::Result<()> {
    // TODO: Change this to argv[1] once possible to set in Wasmtime
    assert_eq!(
        args().last().as_deref(),
        Some("default:http-server/HttpServer.HandleRequest")
    );
    let HttpRequest { body, .. } =
        rmp_serde::from_read(stdin().lock()).context("failed to read http request")?;
    let res = rmp_serde::to_vec(&HttpResponse {
        body: [format!("[{}", env!("CARGO_PKG_NAME")).as_bytes(), &body].join("]".as_bytes()),
        ..Default::default()
    })
    .context("failed to serialize response")?;
    let mut stdout = stdout().lock();
    stdout.write_all(&res).context("failed to write response")?;
    stdout.flush().context("failed to flush")?;
    Ok(())
}
