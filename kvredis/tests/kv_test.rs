use wasmbus_rpc::{provider::prelude::Context, RpcResult};
use wasmcloud_interface_keyvalue::*;
use wasmcloud_test_util::{
    check, check_eq,
    provider_test::{run_tests, test_provider, ProviderProcess, TestFunc},
};

// this shows up as one test case when run with 'cargo test'.
// run with 'cargo test -- --nocapture' to see per-function test case output
#[tokio::test]
async fn test_main() -> Result<(), Box<dyn std::error::Error>> {
    // list of test cases: name and function
    let tests: Vec<(&'static str, TestFunc)> = vec![
        ("health_check", || Box::pin(health_check())),
        ("get_set", || Box::pin(get_set())),
        ("contains_del", || Box::pin(contains_del())),
        ("incr", || Box::pin(incr())),
        ("lists", || Box::pin(lists())),
        ("sets", || Box::pin(sets())),
    ];

    // Each of the test functions below are self-contained, and use independent keys,
    // so it should be possible to run the test cases in any order,
    // or in parallel. A future version of 'run_tests' may use
    // different ordering or scheduling strategies.
    let (passed, total) = run_tests(tests).await?;
    if passed != total {
        log::error!("Not all tests passed: ({}/{})", passed, total);
    }

    Ok(())
}

// syntactic sugar for set
async fn set<T1: ToString, T2: ToString>(
    kv: &KeyValueSender<'_, ProviderProcess>,
    ctx: &Context,
    key: T1,
    value: T2,
) -> RpcResult<()> {
    kv.set(
        ctx,
        &SetRequest {
            key: key.to_string(),
            value: value.to_string(),
            ..Default::default()
        },
    )
    .await
}

/// test that health check returns healthy
async fn health_check() -> RpcResult<()> {
    let prov = test_provider().await;

    // health check
    let hc = prov.health_check().await;
    check!(hc.is_ok())?;
    Ok(())
}

/// get and set
async fn get_set() -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::new(prov);
    let ctx = Context::default();

    const KEY: &str = "t_get_set";
    const VALUE: &str = "Alice";
    // clear test value in case it's leftover from previous test run
    let _ = kv.del(&ctx, KEY).await?;

    let get_resp = kv.get(&ctx, KEY).await?;
    check_eq!(get_resp.exists, false)?;

    set(&kv, &ctx, KEY, VALUE).await?;

    let get_resp = kv.get(&ctx, KEY).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), VALUE)?;

    let _ = kv.del(&ctx, KEY).await?;

    log::debug!("done!!!!");

    Ok(())
}

/// contains and del
async fn contains_del() -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::new(prov);
    let ctx = Context::default();

    const KEY: &str = "t_contains";
    const VALUE: &str = "Bob";
    // clear test value in case it's leftover from previous test run
    let _ = kv.del(&ctx, KEY).await?;

    let empty = kv.contains(&ctx, KEY).await?;
    check_eq!(empty, false)?;

    set(&kv, &ctx, KEY, VALUE).await?;

    let not_empty = kv.contains(&ctx, KEY).await?;
    check!(not_empty)?;

    let _ = kv.del(&ctx, KEY).await?;

    let empty = kv.contains(&ctx, KEY).await?;
    check!(!empty)?;

    Ok(())
}

/// increment (positive and negative)
async fn incr() -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::new(prov);
    let ctx = Context::default();

    const KEY: &str = "t_incr";
    const VALUE: &str = "0";
    // clear test value in case it's leftover from previous test run
    let _ = kv.del(&ctx, KEY).await?;

    set(&kv, &ctx, KEY, VALUE).await?;

    let get_resp = kv.get(&ctx, KEY).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), "0")?;

    kv.increment(
        &ctx,
        &IncrementRequest {
            key: KEY.to_string(),
            value: 25,
        },
    )
    .await?;

    let get_resp = kv.get(&ctx, KEY).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), "25")?;

    kv.increment(
        &ctx,
        &IncrementRequest {
            key: KEY.to_string(),
            value: -5,
        },
    )
    .await?;

    let get_resp = kv.get(&ctx, KEY).await?;
    check!(get_resp.exists)?;
    check_eq!(get_resp.value.as_str(), "20")?;

    Ok(())
}

/// list : list_clear, list_add, list_del, list_range
async fn lists() -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::new(prov);
    let ctx = Context::default();

    const KEY: &str = "t_list";

    // clear test value in case it's leftover from previous test run
    let _ = kv.list_clear(&ctx, KEY).await?;
    let has_list = kv.contains(&ctx, KEY).await?;
    check_eq!(has_list, false)?;

    for (i, name) in ["apple", "banana", "peach"].iter().enumerate() {
        let n: u32 = kv
            .list_add(
                &ctx,
                &ListAddRequest {
                    list_name: KEY.to_string(),
                    value: name.to_string(),
                },
            )
            .await?;
        check_eq!(n, i as u32 + 1)?;
    }
    let has_list = kv.contains(&ctx, KEY).await?;
    check_eq!(has_list, true)?;

    let present = kv
        .list_del(
            &ctx,
            &ListDelRequest {
                list_name: KEY.to_string(),
                value: "banana".to_string(),
            },
        )
        .await?;
    check_eq!(present, true)?;

    let present = kv
        .list_del(
            &ctx,
            &ListDelRequest {
                list_name: KEY.to_string(),
                value: "watermelon".to_string(),
            },
        )
        .await?;
    check_eq!(present, false)?;

    let values = kv
        .list_range(
            &ctx,
            &ListRangeRequest {
                list_name: KEY.to_string(),
                start: 0,
                stop: 100,
            },
        )
        .await?;
    check_eq!(values.len(), 2)?;
    check_eq!(values.get(0).unwrap(), "apple")?;
    check_eq!(values.get(1).unwrap(), "peach")?;

    let _ = kv.list_clear(&ctx, KEY).await?;

    let has_list = kv.contains(&ctx, KEY).await?;
    check_eq!(has_list, false)?;

    Ok(())
}

/// sets : set_add, set_del, set_union, set_intersect, set_clear
async fn sets() -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::new(prov);
    let ctx = Context::default();

    const KEY1: &str = "t_set1";
    let _ = kv.set_clear(&ctx, KEY1);
    const KEY2: &str = "t_set2";
    let _ = kv.set_clear(&ctx, KEY2);
    const KEY3: &str = "t_set3";
    let _ = kv.set_clear(&ctx, KEY3);

    let n: u32 = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: KEY1.to_string(),
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
                set_name: KEY1.to_string(),
                value: "Bob".to_string(),
            },
        )
        .await?;
    check_eq!(n, 1)?;

    let n: u32 = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: KEY1.to_string(),
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
                set_name: KEY1.to_string(),
                value: "Carol".to_string(),
            },
        )
        .await?;
    check_eq!(n, 0)?;

    let _ = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: KEY2.into(),
                value: "Alice".into(),
            },
        )
        .await?;

    let _ = kv
        .set_add(
            &ctx,
            &SetAddRequest {
                set_name: KEY3.into(),
                value: "Daria".into(),
            },
        )
        .await?;

    // intersection
    //
    let inter_12 = kv
        .set_intersection(&ctx, &vec![KEY1.to_string(), KEY2.to_string()])
        .await?;
    check_eq!(inter_12.len(), 1)?;
    check_eq!(inter_12.get(0).unwrap(), "Alice")?;

    let inter_123 = kv
        .set_intersection(
            &ctx,
            &vec![KEY1.to_string(), KEY2.to_string(), KEY3.to_string()],
        )
        .await?;
    check_eq!(inter_123.len(), 0)?;

    // union
    //
    let union_123 = kv
        .set_union(
            &ctx,
            &vec![KEY1.to_string(), KEY2.to_string(), KEY3.to_string()],
        )
        .await?;
    check_eq!(union_123.len(), 4)?;
    // these could be in any order
    check!(union_123.contains(&"Alice".to_string()))?;
    check!(union_123.contains(&"Bob".to_string()))?;
    check!(union_123.contains(&"Carol".to_string()))?;
    check!(union_123.contains(&"Daria".to_string()))?;

    // query
    //
    let q = kv.set_query(&ctx, KEY1).await?;
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
                set_name: KEY1.into(),
                value: "Alice".into(),
            },
        )
        .await;
    let q = kv.set_query(&ctx, KEY1).await?;
    check_eq!(q.len(), 2)?;

    // clear
    //
    // clear should work the first time, returning true
    let has_key1 = kv.set_clear(&ctx, KEY1).await?;
    check_eq!(has_key1, true)?;

    // and return false the second time after it's been removed
    let has_key1 = kv.set_clear(&ctx, KEY1).await?;
    check_eq!(has_key1, false)?;

    Ok(())
}
