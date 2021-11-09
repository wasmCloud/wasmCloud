//! unit test of httpserver
//!
//! This loads the httpserver provider, links it to a mock actor (below),
//! and issues a GET and a PUT request. The actor receives the request and responds
//! with json summary of the request. (similar to what the example Echo actor does)
//!
//! If the test fails with an error
//!    error creating server listener: Address already in use
//! it's because the listening port (set on provider_test_config.toml) is in use.
//! If the only listener on that port is this test, you may have another test process
//! running because a previous run didn't shut down cleanly
//! (this can happen if it failed failed with a panic error).
//! If you're on linux or macos:
//!     Check the output of `ps ax | grep httpserver`,
//!     If it has one or more processes called 'target/debug/httpserver', they're from this test.
//!     Try `killall httpserver` to kill them.
//!
use std::time::Instant;
use wasmbus_rpc::core::InvocationResponse;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_httpserver::*;
use wasmcloud_test_util::{
    check,
    cli::print_test_results,
    provider_test::test_provider,
    testing::{TestOptions, TestResult},
};
#[allow(unused_imports)]
use wasmcloud_test_util::{run_selected, run_selected_spawn};

/// HTTP host and port for this test.
/// Port number should match value in provider_test_config.toml
const SERVER_UNDER_TEST: &str = "http://localhost:9000";

/// number of http requests in this test
const NUM_RPC: u32 = 5;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_all() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let opts = TestOptions::default();

    // launch the mock actor thread
    let join = mock_echo_actor(NUM_RPC).await;

    let res = run_selected_spawn!(&opts, health_check, send_http, send_http_body, test_timeout);
    print_test_results(&res);

    let passed = res.iter().filter(|tr| tr.passed).count();
    let total = res.len();
    assert_eq!(passed, total, "{} passed out of {}", passed, total);

    // check that the thread didn't end early
    let completed = join.await??;
    assert_eq!(completed, NUM_RPC);

    // ask the provider to shut down
    let provider = test_provider().await;
    let _ = provider.shutdown().await;
    Ok(())
}

/// test that health check returns healthy
async fn health_check(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // health check
    let hc = prov.health_check().await;
    check!(hc.is_ok())?;
    Ok(())
}

/// This mock actor runs in a separate thread, listening for rpc requests.
/// It responds to http requests by echoing the response in json.
/// The thread quits if the number of expected messages has been completed,
/// or if there was any error.
async fn mock_echo_actor(num_requests: u32) -> tokio::task::JoinHandle<RpcResult<u32>> {
    use futures::StreamExt;
    use wasmbus_rpc::rpc_topic;
    use wasmbus_rpc::{core::Invocation, deserialize, serialize};

    tokio::spawn(async move {
        let mut completed = 0u32;

        if let Err::<(), RpcError>(e) = {
            let prov = test_provider().await;
            let topic = rpc_topic(&prov.origin(), &prov.host_data.lattice_rpc_prefix);
            // subscribe() returns a Stream of nats messages
            let (_sid, mut sub) = prov
                .nats_client
                .subscribe(topic)
                .await
                .map_err(|e| RpcError::Nats(e.to_string()))?;
            while completed < num_requests {
                let msg = match sub.next().await {
                    None => break,
                    Some(msg) => msg,
                };

                let inv: Invocation = deserialize(&msg.payload)?;
                if &inv.operation != "HttpServer.HandleRequest" {
                    eprintln!("Unexpected method received by actor: {}", &inv.operation);
                    break;
                }
                let http_req: HttpRequest = deserialize(&inv.msg)?;

                // for timeout test, denoted by "sleep" in the path, wait too long to send response
                if http_req.path.contains("sleep") {
                    eprintln!("httpserver /sleep test: expect timeouts in log");
                    tokio::time::sleep(std::time::Duration::from_millis(4000)).await;
                }
                let body = serde_json::to_vec(&serde_json::json!({
                    "msg_id": completed,
                    "method": http_req.method,
                    "path": http_req.path,
                    "query": http_req.query_string,
                    // compute hash of body, to confirm they were sent correctly.
                    // no need to send it back, since serde json doesn't do well with byte arrays anyway
                    "body_len": http_req.body.len(),
                    "body_hash": hash(&http_req.body),
                }))
                .map_err(|e| RpcError::Ser(e.to_string()))?;
                let http_resp = HttpResponse {
                    body,
                    ..Default::default()
                };
                let buf = serialize(&http_resp)?;
                if let Some(ref reply_to) = msg.reply_to {
                    let ir = InvocationResponse {
                        error: None,
                        invocation_id: inv.id,
                        msg: buf,
                    };
                    prov.rpc_client.publish(reply_to, &serialize(&ir)?).await?;
                }
                completed += 1;
            }
            Ok(())
        } {
            eprintln!("mock_actor got error: {}. quitting actor thread", e);
        }
        Ok(completed)
    })
}

async fn send_http(_: &TestOptions) -> RpcResult<()> {
    type JsonData = std::collections::HashMap<String, serde_json::Value>;

    // send GET request
    let client = reqwest::Client::new();
    let start_time = Instant::now();
    let resp = client
        .get(&format!("{}/abc", SERVER_UNDER_TEST))
        .send()
        .await
        .map_err(|e| RpcError::Other(e.to_string()))?;
    let elapsed = start_time.elapsed();
    eprintln!("GET /abc returned in {} ms", &elapsed.as_millis());
    assert_eq!(resp.status().as_u16(), 200);

    let body = resp
        .json::<JsonData>()
        .await
        .map_err(|e| RpcError::Deser(e.to_string()))?;
    assert_eq!(body.get("method").unwrap().as_str(), Some("GET"));
    assert_eq!(body.get("path").unwrap().as_str(), Some("/abc"));

    // send GET request with query
    let client = reqwest::Client::new();
    let start_time = Instant::now();
    let resp = client
        .get(&format!("{}/def?name=Carol&thing=one", SERVER_UNDER_TEST))
        .send()
        .await
        .map_err(|e| RpcError::Other(e.to_string()))?;
    let elapsed = start_time.elapsed();
    eprintln!("GET /def returned in {} ms", &elapsed.as_millis());
    assert_eq!(resp.status().as_u16(), 200);

    let body = resp
        .json::<JsonData>()
        .await
        .map_err(|e| RpcError::Deser(e.to_string()))?;
    assert_eq!(body.get("method").unwrap().as_str(), Some("GET"));
    assert_eq!(body.get("path").unwrap().as_str(), Some("/def"));
    assert_eq!(
        body.get("query").unwrap().as_str(),
        Some("name=Carol&thing=one")
    );
    Ok(())
}

async fn send_http_body(_: &TestOptions) -> RpcResult<()> {
    type JsonData = std::collections::HashMap<String, serde_json::Value>;

    // send POST request with empty body
    let client = reqwest::Client::new();
    let start_time = Instant::now();
    let resp = client
        .post(&format!("{}/1", SERVER_UNDER_TEST))
        .send()
        .await
        .map_err(|e| RpcError::Other(e.to_string()))?;
    let elapsed = start_time.elapsed();
    eprintln!("POST /1 returned in {} ms", &elapsed.as_millis());
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp
        .json::<JsonData>()
        .await
        .map_err(|e| RpcError::Deser(e.to_string()))?;
    assert_eq!(body.get("method").unwrap().as_str(), Some("POST"));
    assert_eq!(body.get("path").unwrap().as_str(), Some("/1"));
    assert_eq!(body.get("body_len").unwrap().as_i64(), Some(0));

    // send PUT request with binary(non-ascii) data
    let mut blob = [0u8; 700];
    for (i, item) in blob.iter_mut().enumerate() {
        *item = (i % 256) as u8;
    }
    let expected_hash = hash(&blob);

    let client = reqwest::Client::new();
    let start_time = Instant::now();
    let resp = client
        .put(&format!("{}/2", SERVER_UNDER_TEST))
        .body(blob.to_vec())
        .send()
        .await
        .map_err(|e| RpcError::Other(e.to_string()))?;
    let elapsed = start_time.elapsed();
    eprintln!("POST /2 returned in {} ms", &elapsed.as_millis());
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp
        .json::<JsonData>()
        .await
        .map_err(|e| RpcError::Deser(e.to_string()))?;
    assert_eq!(body.get("path").unwrap().as_str(), Some("/2"));
    assert_eq!(body.get("method").unwrap().as_str(), Some("PUT"));
    assert_eq!(
        body.get("body_len").unwrap().as_u64(),
        Some(blob.len() as u64)
    );
    assert_eq!(
        body.get("body_hash").unwrap().as_str(),
        Some(expected_hash.as_str())
    );

    Ok(())
}

async fn test_timeout(_: &TestOptions) -> RpcResult<()> {
    // send GET request with "sleep" in the path to trigger the actor to wait too long
    let client = reqwest::Client::new();
    let start_time = Instant::now();
    let resp = client
        .get(&format!("{}/sleep", SERVER_UNDER_TEST))
        .send()
        .await;
    let elapsed = start_time.elapsed();
    eprintln!("GET /sleep returned in {} ms", &elapsed.as_millis());
    eprintln!("DEBUG /sleep response after timeout: {:#?}", &resp);

    assert!(resp.is_ok(), "expect ok response");
    let resp = resp.unwrap();
    assert_eq!(resp.status().as_u16(), 503, "expected 503 timeout error");

    Ok(())
}

/// compute hash of data
fn hash(buf: &[u8]) -> String {
    use blake2::{Blake2b, Digest};
    let mut hasher = Blake2b::new();
    hasher.update(buf);
    let res = hasher.finalize();
    format!("{:x}", res)
}
