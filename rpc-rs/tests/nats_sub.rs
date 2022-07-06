//! test nats subscriptions (queue and non-queue) with rpc_client
#![cfg(test)]

use std::{str::FromStr, sync::Arc, time::Duration};

use tracing::{debug, error};
use wascap::prelude::KeyPair;
use wasmbus_rpc::{
    error::{RpcError, RpcResult},
    rpc_client::RpcClient,
};

const ONE_SEC: Duration = Duration::from_secs(1);
const THREE_SEC: Duration = Duration::from_secs(3);
const TEST_NATS_ADDR: &str = "nats://127.0.0.1:4222";
const HOST_ID: &str = "HOST_test_nats_sub";

fn nats_url() -> String {
    if let Ok(addr) = std::env::var("NATS_URL") {
        addr
    } else {
        TEST_NATS_ADDR.to_string()
    }
}

fn is_demo() -> bool {
    nats_url().contains("demo.nats.io")
}

/// create async nats client for test (sender or receiver)
/// Parameter is optional RPC timeout
async fn make_client(timeout: Option<Duration>) -> RpcResult<RpcClient> {
    let nats_url = nats_url();
    let server_addr = async_nats::ServerAddr::from_str(&nats_url).unwrap();
    let nc = async_nats::ConnectOptions::default()
        .connect(server_addr)
        .await
        .map_err(|e| {
            RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_url, e))
        })?;

    let key_pair = KeyPair::new_user();
    let client = RpcClient::new(nc, HOST_ID.to_string(), timeout, Arc::new(key_pair));
    Ok(client)
}

async fn listen(client: RpcClient, subject: &str, pattern: &str) -> tokio::task::JoinHandle<u64> {
    use futures::StreamExt;

    let subject = subject.to_string();
    let pattern = pattern.to_string();
    let nc = client.client();

    let pattern = regex::Regex::new(&pattern).unwrap();
    let mut sub = nc.subscribe(subject.clone()).await.expect("subscriber");

    tokio::task::spawn(async move {
        let mut count: u64 = 0;
        while let Some(msg) = sub.next().await {
            let payload = String::from_utf8_lossy(&msg.payload);
            if !pattern.is_match(payload.as_ref()) && &payload != "exit" {
                println!("ERROR: payload on {}: {}", subject, &payload);
            }
            if let Some(reply_to) = msg.reply {
                client.publish(reply_to, b"ok".to_vec()).await.expect("reply");
            }
            if payload == "exit" {
                break;
            }
            count += 1;
        }
        println!("received {} message(s)", count);
        count
    })
}

async fn listen_bin(client: RpcClient, subject: &str) -> tokio::task::JoinHandle<u64> {
    use futures::StreamExt;
    let subject = subject.to_string();
    let nc = client.client();

    let mut sub = nc.subscribe(subject.clone()).await.expect("subscriber");
    tokio::task::spawn(async move {
        let mut count: u64 = 0;
        while let Some(msg) = sub.next().await {
            let size = msg.payload.len();
            let response = format!("{}", size);
            if let Some(reply_to) = msg.reply {
                if let Err(e) = nc.publish(reply_to, response.as_bytes().to_vec().into()).await {
                    error!("error publishing subscriber response: {}", e);
                }
            }
            count += 1;
            if size == 1 {
                break;
            }
        }
        let _ = sub.unsubscribe().await;
        debug!("listen_bin exiting with count {}", count);
        count
    })
}

async fn listen_queue(
    client: RpcClient,
    subject: &str,
    queue: &str,
    pattern: &str,
) -> tokio::task::JoinHandle<u64> {
    use futures::StreamExt;
    let subject = subject.to_string();
    let queue = queue.to_string();
    let pattern = pattern.to_string();
    let nc = client.client();

    tokio::task::spawn(async move {
        let mut count: u64 = 0;
        let pattern = regex::Regex::new(&pattern).unwrap();
        let mut sub = nc
            .queue_subscribe(subject.clone(), queue.clone())
            .await
            .expect("group subscriber");
        while let Some(msg) = sub.next().await {
            let payload = String::from_utf8_lossy(&msg.payload);
            if !pattern.is_match(payload.as_ref()) && &payload != "exit" {
                debug!("ERROR: payload on {}: {}", &subject, &payload);
                break;
            }
            if let Some(reply_to) = msg.reply {
                debug!("listener {} replying ok", &subject);
                client.publish(reply_to, b"ok".to_vec()).await.expect("reply");
            }
            if &payload == "exit" {
                let _ = sub.unsubscribe().await;
                break;
            }
            count += 1;
        }
        println!("subscriber '{}' exiting count={}", &subject, count);
        count
    })
}

#[tokio::test]
async fn simple_sub() -> Result<(), Box<dyn std::error::Error>> {
    // create unique subscription name for this test
    let sub_name = uuid::Uuid::new_v4().to_string();

    let topic = format!("one_{}", &sub_name);
    let l1 = listen(make_client(None).await?, &topic, "^abc").await;

    let sender = make_client(None).await.expect("creating sender");
    sender.publish(topic.clone(), b"abc".to_vec()).await.expect("send");
    sender.publish(topic, b"exit".to_vec()).await.expect("send");
    let val = l1.await.expect("join");

    assert_eq!(val, 1);
    Ok(())
}

/// send large messages - this uses request() and does not test chunking
#[tokio::test]
async fn test_message_size() -> Result<(), Box<dyn std::error::Error>> {
    // create unique subscription name for this test
    let sub_name = uuid::Uuid::new_v4().to_string();

    let topic = format!("bin_{}", &sub_name);
    let l1 = listen_bin(make_client(Some(THREE_SEC)).await?, &topic).await;

    let mut pass_count = 0;
    let sender = make_client(Some(THREE_SEC)).await.expect("creating bin sender");
    //  messages sizes to test
    let test_sizes = if is_demo() {
        // if using 'demo.nats.io' as the test server,
        // don't abuse it by running this test with very large sizes
        //
        // The last size must be 1 to signal to listen_bin to exit
        &[10u32, 25, 100, 200, 500, 1000, 1]
    } else {
        // The last size must be 1 to signal to listen_bin to exit
        &[10u32, 25, 500, 10_000, 800_000, 1_000_000, 1]
    };
    for size in test_sizes.iter() {
        let mut data = Vec::with_capacity(*size as usize);
        data.resize(*size as usize, 255u8);
        let resp = match tokio::time::timeout(THREE_SEC, sender.request(topic.clone(), data)).await
        {
            Ok(Ok(result)) => result,
            Ok(Err(rpc_err)) => {
                eprintln!("send error on msg size {}: {}", *size, rpc_err);
                continue;
            }
            Err(timeout_err) => {
                eprintln!(
                    "rpc timeout: sending msg of size {}: {}",
                    *size, timeout_err
                );
                continue;
            }
        };
        let sbody = String::from_utf8_lossy(&resp);
        let received_size = sbody.parse::<u32>().expect("response contains int size");
        if *size == received_size {
            eprintln!("PASS: message_size: {}", size);
            pass_count += 1;
        } else {
            eprintln!("FAIL: message_size: {}, got: {}", size, received_size);
        }
    }
    assert_eq!(pass_count, test_sizes.len(), "some size tests did not pass");
    let val = l1.await.expect("join");
    assert_eq!(
        val as usize,
        test_sizes.len(),
        "some messages were not received"
    );
    Ok(())
}

async fn sleep(millis: u64) {
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

fn check_ok(data: Vec<u8>) -> Result<(), RpcError> {
    let text = String::from_utf8_lossy(&data);
    if &text == "ok" {
        Ok(())
    } else {
        Err(RpcError::Other(format!("Invalid response: {}", &text)))
    }
}

#[tokio::test]
async fn queue_sub() -> Result<(), Box<dyn std::error::Error>> {
    // in this test, there are two queue subscribers.
    // on topic "one..." with the same queue group X,
    // and one queue subscriber on topic "two..." with queue group X
    //
    // We expect
    //   topic "one" messages split between first two listeners
    //   topic "two" messages only sent to the third listener
    // This confirms that publishing to queue subscription divides the load,
    // and also confirms that a queue group name ('X') is only applicable
    // within a topic.
    let sub_name = uuid::Uuid::new_v4().to_string();
    let topic_one = format!("one_{}", &sub_name);
    let topic_two = format!("two_{}", &sub_name);

    let queue_name = uuid::Uuid::new_v4().to_string();

    let thread1 = listen_queue(make_client(None).await?, &topic_one, &queue_name, "^one").await;
    let thread2 = listen_queue(make_client(None).await?, &topic_one, &queue_name, "^one").await;
    let thread3 = listen_queue(make_client(None).await?, &topic_two, &queue_name, "^two").await;
    sleep(200).await;

    let sender = make_client(None).await?;
    const SPLIT_TOTAL: usize = 6;
    const SINGLE_TOTAL: usize = 6;
    for _ in 0..SPLIT_TOTAL {
        check_ok(sender.request(topic_one.clone(), b"one".to_vec()).await?)?;
    }
    for _ in 0..SINGLE_TOTAL {
        check_ok(sender.request(topic_two.clone(), b"two".to_vec()).await?)?;
    }
    check_ok(sender.request(topic_one.clone(), b"exit".to_vec()).await?)?;
    check_ok(sender.request(topic_one.clone(), b"exit".to_vec()).await?)?;
    check_ok(sender.request(topic_two.clone(), b"exit".to_vec()).await?)?;

    let v3 = wait_for(thread3, ONE_SEC).await??;
    let v2 = wait_for(thread2, ONE_SEC).await??;
    let v1 = wait_for(thread1, ONE_SEC).await??;

    assert_eq!(v1 + v2, SPLIT_TOTAL as u64, "no loss in queue");
    assert_eq!(v3, SINGLE_TOTAL as u64, "no overlap between queues");
    Ok(())
}

async fn wait_for<O, F: futures::Future<Output = O>>(
    f: F,
    timeout: Duration,
) -> Result<O, Box<dyn std::error::Error>> {
    let res: O = tokio::time::timeout(timeout, f).await?;
    Ok(res)
}
