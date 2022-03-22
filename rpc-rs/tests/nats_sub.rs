//! test nats subscriptions (queue and non-queue) with rpc_client
#![cfg(test)]

const THREE_SEC: Duration = Duration::from_secs(3);

use std::{str::FromStr as _, time::Duration};

use tracing::debug;
use wasmbus_rpc::{
    error::{RpcError, RpcResult},
    rpc_client::RpcClient,
};

//const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";
const TEST_NATS_ADDR: &str = "demo.nats.io";
const LATTICE_PREFIX: &str = "test_nats_sub";
const HOST_ID: &str = "HOST_test_nats_sub";

/// create async nats client for test (sender or receiver)
async fn make_client() -> RpcResult<RpcClient> {
    let server_addr = wasmbus_rpc::anats::ServerAddress::from_str(TEST_NATS_ADDR).unwrap();
    let nc = wasmbus_rpc::anats::Options::default()
        .max_reconnects(None)
        .connect(vec![server_addr])
        .await
        .map_err(|e| {
            RpcError::ProviderInit(format!(
                "nats connection to {} failed: {}",
                TEST_NATS_ADDR, e
            ))
        })?;
    let kp = wascap::prelude::KeyPair::new_user();
    let client = RpcClient::new(
        nc,
        LATTICE_PREFIX,
        kp,
        HOST_ID.to_string(),
        Some(Duration::from_secs(5)),
    );
    Ok(client)
}

async fn listen(client: RpcClient, subject: &str, pattern: &str) -> tokio::task::JoinHandle<u64> {
    let subject = subject.to_string();
    let pattern = pattern.to_string();
    let nc = client.get_async().unwrap();

    let pattern = regex::Regex::new(&pattern).unwrap();
    let sub = nc.subscribe(&subject).await.expect("subscriber");

    tokio::task::spawn(async move {
        let mut count: u64 = 0;
        while let Some(msg) = sub.next().await {
            let payload = String::from_utf8_lossy(&msg.data);
            if !pattern.is_match(payload.as_ref()) && &payload != "exit" {
                println!("ERROR: payload on {}: {}", &subject, &payload);
            }
            if let Some(reply_to) = msg.reply {
                client.publish(&reply_to, b"ok").await.expect("reply");
            }
            if payload == "exit" {
                break;
            }
            count += 1;
        }
        println!("exiting: {}", count);
        let _ = sub.close().await;
        count
    })
}

async fn listen_bin(client: RpcClient, subject: &str) -> tokio::task::JoinHandle<u64> {
    let subject = subject.to_string();
    let nc = client.get_async().unwrap();

    let sub = nc.subscribe(&subject).await.expect("subscriber");
    tokio::task::spawn(async move {
        let mut count: u64 = 0;
        println!("listening subj: {}", &subject);
        while let Some(msg) = sub.next().await {
            let size = msg.data.len();
            let response = format!("{}", size);
            if let Some(reply_to) = msg.reply {
                client
                    .publish(&reply_to, response.as_bytes())
                    .await
                    .expect("reply");
            }
            count += 1;
            if size == 1 {
                break;
            }
        }
        let _ = sub.close().await;
        println!("exiting: {}", count);
        count
    })
}

async fn listen_queue(
    client: RpcClient,
    subject: &str,
    queue: &str,
    pattern: &str,
) -> tokio::task::JoinHandle<u64> {
    let subject = subject.to_string();
    let queue = queue.to_string();
    let pattern = pattern.to_string();
    let nc = client.get_async().unwrap();

    tokio::task::spawn(async move {
        let mut count: u64 = 0;
        let pattern = regex::Regex::new(&pattern).unwrap();
        let sub = nc
            .queue_subscribe(&subject, &queue)
            .await
            .expect("group subscriber");
        debug!("listening subj: {} queue: {}", &subject, &queue);
        while let Some(msg) = sub.next().await {
            let payload = String::from_utf8_lossy(&msg.data);
            if !pattern.is_match(payload.as_ref()) && &payload != "exit" {
                debug!("ERROR: payload on {}: {}", &subject, &payload);
                break;
            }
            if let Some(reply_to) = msg.reply {
                debug!("listener {} replying ok", &subject);
                client.publish(&reply_to, b"ok").await.expect("reply");
            }
            if &payload == "exit" {
                debug!("listener {} received 'exit'", &subject);
                //let _ = sub.close().await;
                break;
            }
            count += 1;
        }
        println!("listener {} exiting with count {}", &subject, count);
        count
    })
}

#[tokio::test]
async fn simple_sub() -> Result<(), Box<dyn std::error::Error>> {
    // create unique subscription name for this test
    let sub_name = uuid::Uuid::new_v4().to_string();

    let topic = format!("one_{}", &sub_name);
    let l1 = listen(make_client().await?, &topic, "^abc").await;

    let sender = make_client().await.expect("creating sender");
    sender.publish(&topic, b"abc").await.expect("send");
    sender.publish(&topic, b"exit").await.expect("send");
    let val = l1.await.expect("join");

    assert_eq!(val, 1);
    Ok(())
}

/// send large messages to find size limits
#[tokio::test]
async fn test_message_size() -> Result<(), Box<dyn std::error::Error>> {
    if env_logger::try_init().is_err() {};
    // create unique subscription name for this test
    let sub_name = uuid::Uuid::new_v4().to_string();

    let topic = format!("bin_{}", &sub_name);
    let l1 = listen_bin(make_client().await?, &topic).await;

    let mut pass_count = 0;
    let sender = make_client().await.expect("creating bin sender");
    const TEST_SIZES: &[u32] = &[
        100, 200,
        // NOTE: if using 'demo.nats.io' as the test server,
        // don't abuse it by running this test - only use larger sizes
        // if testing against a local nats server.
        //
        // 100_000, 200_000, 300_000, 400_000, 500_000, 600_000, 700_000, 800_000, 900_000,
        //1_000_000, (1024 * 1024),
        // The last size must be 1: signal to listen_bin to exit
        1,
    ];
    for size in TEST_SIZES.iter() {
        let mut data = Vec::with_capacity(*size as usize);
        data.resize(*size as usize, 255u8);
        let resp =
            match tokio::time::timeout(Duration::from_millis(2000), sender.request(&topic, &data))
                .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(rpc_err)) => {
                    eprintln!("rpc send error on msg size {}: {}", *size, rpc_err);
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
    assert_eq!(pass_count, TEST_SIZES.len(), "some size tests did not pass");
    let val = l1.await.expect("join");
    assert_eq!(
        val as usize,
        TEST_SIZES.len(),
        "some messages were not received"
    );
    Ok(())
}

async fn sleep(millis: u64) {
    tokio::time::sleep(tokio::time::Duration::from_millis(millis)).await;
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
    let _ = env_logger::try_init();
    let sub_name = uuid::Uuid::new_v4().to_string();
    let topic_one = format!("one_{}", &sub_name);
    let topic_two = format!("two_{}", &sub_name);

    let queue_name = uuid::Uuid::new_v4().to_string();

    let thread1 = listen_queue(make_client().await?, &topic_one, &queue_name, "^one").await;
    let thread2 = listen_queue(make_client().await?, &topic_one, &queue_name, "^one").await;
    let thread3 = listen_queue(make_client().await?, &topic_two, &queue_name, "^two").await;
    sleep(2000).await;

    let sender = make_client().await?;
    const SPLIT_TOTAL: usize = 6;
    const SINGLE_TOTAL: usize = 6;
    for _ in 0..SPLIT_TOTAL {
        check_ok(sender.request(&topic_one, b"one").await?)?;
    }
    for _ in 0..SINGLE_TOTAL {
        check_ok(sender.request(&topic_two, b"two").await?)?;
    }
    check_ok(sender.request(&topic_one, b"exit").await?)?;
    check_ok(sender.request(&topic_one, b"exit").await?)?;
    check_ok(sender.request(&topic_two, b"exit").await?)?;

    let v3 = wait_for(thread3, THREE_SEC).await??;
    let v2 = wait_for(thread2, THREE_SEC).await??;
    let v1 = wait_for(thread1, THREE_SEC).await??;

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
