use tokio::sync::oneshot;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_messaging::*;
use wasmcloud_test_util::{
    check,
    cli::print_test_results,
    provider_test::test_provider,
    run_selected_spawn,
    testing::{TestOptions, TestResult},
};

#[tokio::test]
async fn run_all() {
    let opts = TestOptions::default();
    let res = run_selected_spawn!(opts, health_check, send_request, send_publish);
    print_test_results(&res);

    let passed = res.iter().filter(|tr| tr.passed).count();
    let total = res.len();
    assert_eq!(passed, total, "{} passed out of {}", passed, total);

    // try to let the provider shut dowwn gracefully
    let provider = test_provider().await;
    let _ = provider.shutdown().await;
}

/// test that health check returns healthy
async fn health_check(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // health check
    let hc = prov.health_check().await;
    check!(hc.is_ok())?;
    Ok(())
}

/// create a thread to subscribe to a topic, respond to 'count' messages, and exit
async fn make_responder(
    topic: String,
    count: usize,
) -> (oneshot::Receiver<()>, tokio::task::JoinHandle<usize>) {
    use futures::StreamExt as _;
    let (tx, rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let conn = match async_nats::ConnectOptions::default()
            .connect("127.0.0.1:4222")
            .await
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ERROR: failed to connect test responder to nats: {}", e);
                return 0;
            }
        };
        let mut sub = match conn.subscribe(topic).await {
            Err(e) => {
                eprintln!("ERROR: test failed to subscribe: {}", e);
                return 0;
            }
            Ok(conn) => conn,
        };
        assert!(tx.send(()).is_ok());
        for completed in 0..count {
            let msg = match sub.next().await {
                None => break,
                Some(msg) => msg,
            };
            if let Some(reply) = msg.reply {
                let response = format!("{}:{}", completed, &String::from_utf8_lossy(&msg.payload));
                if let Err(e) = conn.publish(reply, response.into_bytes().into()).await {
                    eprintln!("responder failed replying #{}: {}", &completed, e);
                }
            }
        }
        count
    });
    (rx, join)
}

/// send request and ensure reply is received
async fn send_request(_opt: &TestOptions) -> RpcResult<()> {
    const TEST_SUB_REQ: &str = "test.nats.req";
    let prov = test_provider().await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // create client and ctx
    let client = MessagingSender::via(prov);
    let ctx = Context::default();
    // start responder thread
    let (rx, responder) = make_responder(TEST_SUB_REQ.to_string(), 1).await;
    assert!(rx.await.is_ok());

    let resp = client
        .request(
            &ctx,
            &RequestMessage {
                subject: TEST_SUB_REQ.to_string(),
                body: b"hello".to_vec(),
                timeout_ms: 1000,
            },
        )
        .await?;
    assert_eq!(&resp.body, b"0:hello");

    let completed = responder
        .await
        .map_err(|e| RpcError::Other(format!("request join failure: {}", e)))?;
    assert_eq!(completed, 1usize);
    Ok(())
}

/// send publish
async fn send_publish(_opt: &TestOptions) -> RpcResult<()> {
    const TEST_SUB_PUB: &str = "test.nats.pub";
    let prov = test_provider().await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // start responder thread
    let (rx, responder) = make_responder(TEST_SUB_PUB.to_string(), 1).await;
    assert!(rx.await.is_ok());

    // create client and ctx
    let client = MessagingSender::via(prov);
    let ctx = Context::default();

    client
        .publish(
            &ctx,
            &PubMessage {
                subject: TEST_SUB_PUB.to_string(),
                body: b"hello".to_vec(),
                reply_to: None,
            },
        )
        .await?;
    let completed = responder
        .await
        .map_err(|e| RpcError::Other(format!("request join failure: {}", e)))?;
    assert_eq!(completed, 1usize);

    Ok(())
}
