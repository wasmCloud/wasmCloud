use std::time::Duration;
use tokio::time::sleep;
use wasmbus_rpc::{
    error::{RpcError, RpcResult},
    provider::prelude::Context,
};
use wasmcloud_interface_keyvalue::*;
use wasmcloud_test_util::{
    check, check_eq,
    cli::print_test_results,
    provider_test::{self, log, Config, LogLevel, Provider},
    run_selected_spawn,
    testing::{TestOptions, TestResult},
};

async fn test_provider() -> Provider {
    provider_test::test_provider(
        env!("CARGO_BIN_EXE_kvredis"),
        Config {
            log_level: LogLevel(log::Level::Debug),
            backtrace: true,
            contract_id: "wasmcloud:keyvalue".into(),
            ..Default::default()
        },
    )
    .await
}

#[tokio::test]
async fn run_all() {
    let opts = TestOptions::default();
    let res = run_selected_spawn!(opts, health_check, get_set, contains_del, incr, lists, sets);
    print_test_results(&res);

    let passed = res.iter().filter(|tr| tr.passed).count();
    let total = res.len();
    assert_eq!(passed, total, "{} passed out of {}", passed, total);

    // try to let the provider shut down gracefully
    let provider = test_provider().await;
    let _ = provider.shutdown().await;
}

/// returns a new test key with the given prefix
/// The purpose is to make sure different tests don't collide with each other
fn new_key(prefix: &str) -> String {
    format!("{}_{:x}", prefix, rand::random::<u32>())
}

// syntactic sugar for set
async fn set<T1: ToString, T2: ToString>(
    kv: &KeyValueSender<Provider>,
    ctx: &Context,
    key: T1,
    value: T2,
    exp: u32,
) -> RpcResult<()> {
    kv.set(
        ctx,
        &SetRequest {
            key: key.to_string(),
            value: value.to_string(),
            expires: exp,
        },
    )
    .await
}

/// test that health check returns healthy
async fn health_check(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // health check
    let hc = prov.health_check().await;
    check!(hc.is_ok())?;
    Ok(())
}

/// get and set
async fn get_set(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let key = new_key("get");
    const VALUE: &str = "Alice";

    let get_resp = kv.get(&ctx, &key).await?;
    check_eq!(get_resp.exists, false)?;

    set(&kv, &ctx, &key, VALUE, 0).await?;

    let get_resp = kv.get(&ctx, &key).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), VALUE)?;

    let _ = kv.del(&ctx, &key).await?;

    //With expiration
    set(&kv, &ctx, &key, VALUE, 3).await?; //Will expire after 3 seconds

    sleep(Duration::from_secs(5)).await;

    let get_resp = kv.get(&ctx, &key).await?;
    check!(!get_resp.exists)?;

    tracing::debug!("done!!!!");

    // clean up
    let _ = kv.del(&ctx, &key).await?;
    Ok(())
}

/// contains and del
async fn contains_del(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let key = new_key("contains");
    const VALUE: &str = "Bob";

    let has_key_before_set = kv.contains(&ctx, &key).await?;
    check_eq!(has_key_before_set, false)?;

    set(&kv, &ctx, &key, VALUE, 0).await?;

    let has_key_after_set = kv.contains(&ctx, &key).await?;
    check_eq!(has_key_after_set, true)?;

    let _ = kv.del(&ctx, &key).await?;

    let has_key_after_del = kv.contains(&ctx, &key).await?;
    check_eq!(has_key_after_del, false)?;

    // clean up
    let _ = kv.del(&ctx, &key).await?;
    Ok(())
}

/// increment (positive and negative)
async fn incr(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let key = new_key("incr");
    const VALUE: &str = "0";

    // initialize the counter to zero
    set(&kv, &ctx, &key, VALUE, 0).await?;

    let get_resp = kv.get(&ctx, &key).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), "0")?;

    kv.increment(
        &ctx,
        &IncrementRequest {
            key: key.clone(),
            value: 25,
        },
    )
    .await?;

    let get_resp = kv.get(&ctx, &key).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), "25")?;

    kv.increment(
        &ctx,
        &IncrementRequest {
            key: key.clone(),
            value: -5,
        },
    )
    .await?;

    let get_resp = kv.get(&ctx, &key).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), "20")?;

    // clean up
    let _ = kv.del(&ctx, &key).await?;
    Ok(())
}

/// list : list_clear, list_add, list_del, list_range
async fn lists(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();
    let key = new_key("list");

    let has_list = kv.contains(&ctx, &key).await?;
    check_eq!(has_list, false)?;

    for (i, name) in ["apple", "banana", "peach"].iter().enumerate() {
        let n: u32 = kv
            .list_add(
                &ctx,
                &ListAddRequest {
                    list_name: key.clone(),
                    value: name.to_string(),
                },
            )
            .await?;
        check_eq!(n, i as u32 + 1)?;
    }
    let has_list = kv.contains(&ctx, &key).await?;
    check_eq!(has_list, true)?;

    let present = kv
        .list_del(
            &ctx,
            &ListDelRequest {
                list_name: key.clone(),
                value: "banana".to_string(),
            },
        )
        .await?;
    check_eq!(present, true)?;

    let present = kv
        .list_del(
            &ctx,
            &ListDelRequest {
                list_name: key.clone(),
                value: "watermelon".to_string(),
            },
        )
        .await?;
    check_eq!(present, false)?;

    let values = kv
        .list_range(
            &ctx,
            &ListRangeRequest {
                list_name: key.clone(),
                start: 0,
                stop: 100,
            },
        )
        .await?;
    check_eq!(values.len(), 2)?;
    check_eq!(values.get(0).unwrap(), "apple")?;
    check_eq!(values.get(1).unwrap(), "peach")?;

    let _ = kv.list_clear(&ctx, &key).await?;

    let has_list = kv.contains(&ctx, &key).await?;
    check_eq!(has_list, false)?;

    // clean up
    let _ = kv.list_clear(&ctx, &key).await?;
    Ok(())
}

/// sets : set_add, set_del, set_union, set_intersect, set_clear
async fn sets(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let key1 = new_key("set1");
    let key2 = new_key("set2");
    let key3 = new_key("set3");

    let n: u32 = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: key1.clone(),
                value: "Alice".to_string(),
            },
        )
        .await?;
    check_eq!(n, 1)?;

    // set_add
    //
    let n: u32 = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: key1.clone(),
                value: "Bob".to_string(),
            },
        )
        .await?;
    check_eq!(n, 1)?;

    let n: u32 = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: key1.clone(),
                value: "Carol".to_string(),
            },
        )
        .await?;
    check_eq!(n, 1)?;

    // add on duplicate key should return 0 new members
    let n: u32 = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: key1.clone(),
                value: "Carol".to_string(),
            },
        )
        .await?;
    check_eq!(n, 0)?;

    let _ = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: key2.clone(),
                value: "Alice".into(),
            },
        )
        .await?;

    let _ = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: key3.clone(),
                value: "Daria".into(),
            },
        )
        .await?;

    // intersection
    //
    let inter_12 = kv
        .set_intersection(&ctx, &vec![key1.clone(), key2.clone()])
        .await?;
    check_eq!(inter_12.len(), 1)?;
    check_eq!(inter_12.get(0).unwrap(), "Alice")?;

    let inter_123 = kv
        .set_intersection(&ctx, &vec![key1.clone(), key2.clone(), key3.clone()])
        .await?;
    check_eq!(inter_123.len(), 0)?;

    // union
    //
    let union_123 = kv
        .set_union(&ctx, &vec![key1.clone(), key2.clone(), key3.clone()])
        .await?;
    check_eq!(union_123.len(), 4)?;
    // these could be in any order
    check!(union_123.contains(&"Alice".to_string()))?;
    check!(union_123.contains(&"Bob".to_string()))?;
    check!(union_123.contains(&"Carol".to_string()))?;
    check!(union_123.contains(&"Daria".to_string()))?;

    // query
    //
    let q = kv.set_query(&ctx, &key1).await?;
    check_eq!(q.len(), 3)?;
    // these could be in any order
    check!(q.contains(&"Alice".to_string()))?;
    check!(q.contains(&"Bob".to_string()))?;
    check!(q.contains(&"Carol".to_string()))?;

    // delete
    //
    let _ = kv
        .set_del(
            &ctx,
            &SetDelRequest {
                set_name: key1.clone(),
                value: "Alice".into(),
            },
        )
        .await;
    let q = kv.set_query(&ctx, &key1).await?;
    check_eq!(q.len(), 2)?;

    // clear
    //
    // clear should work the first time, returning true
    let has_key1 = kv.set_clear(&ctx, &key1).await?;
    check_eq!(has_key1, true)?;

    // and return false the second time after it's been removed
    let has_key1 = kv.set_clear(&ctx, &key1).await?;
    check_eq!(has_key1, false)?;

    // clean up
    let _ = kv.set_clear(&ctx, &key1).await?;
    let _ = kv.set_clear(&ctx, &key2).await?;
    let _ = kv.set_clear(&ctx, &key2).await?;
    Ok(())
}
