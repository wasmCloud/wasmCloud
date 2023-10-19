//! Tests kv-vault
//!
use kv_vault_lib::STRING_VALUE_MARKER;
use serde_json::Value;
use wasmbus_rpc::{
    error::{RpcError, RpcResult},
    provider::prelude::Context,
};
use wasmcloud_interface_keyvalue::*;
use wasmcloud_test_util::{
    check, check_eq,
    cli::print_test_results,
    provider_test::{test_provider, Provider},
    run_selected_spawn,
    testing::TestOptions,
};

#[tokio::test]
async fn run_all() {
    let opts = TestOptions::default();

    let res = run_selected_spawn!(
        opts,
        health_check,
        get_set,
        contains_del,
        json_values,
        renewal,
    );
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
    env_logger::try_init().ok();

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let list = kv.set_query(&ctx, "test_get").await?;
    assert_eq!(list.len(), 0, "nothing before write");

    let key = new_key("test_get/get");
    const VALUE: &str = "Alice";

    let get_resp = kv.get(&ctx, &key).await.expect("get non-existent key");
    check_eq!(get_resp.exists, false)?;

    set(&kv, &ctx, &key, &VALUE.to_string())
        .await
        .expect("set key first time");

    let get_resp = kv.get(&ctx, &key).await.expect("get exists");
    check!(get_resp.exists).expect("get exists check");
    check_eq!(get_resp.value.as_str(), VALUE).expect("get exists value check");

    let list = kv.set_query(&ctx, "test_get").await?;
    assert_eq!(list.len(), 1, "list after first set");

    // clean up
    //let _ = kv.del(&ctx, &key).await.expect("delete key");
    Ok(())
}

/// contains and del
async fn contains_del(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let key = new_key("test_cdel/contains");
    const VALUE: &str = "Bob";

    let has_key_before_set = kv.contains(&ctx, &key).await?;
    check_eq!(has_key_before_set, false)?;

    set(&kv, &ctx, &key, VALUE).await?;

    let has_key_after_set = kv.contains(&ctx, &key).await?;

    check_eq!(has_key_after_set, true)?;

    // clean up
    let _ = kv.del(&ctx, &key).await?;
    let has_key_after_del = kv.contains(&ctx, &key).await?;
    check_eq!(has_key_after_del, false)?;

    Ok(())
}

/// tests json serialization of values
async fn json_values(_opt: &TestOptions) -> RpcResult<()> {
    use std::collections::HashMap;

    let prov = test_provider().await;
    env_logger::try_init().ok();

    let env_file = std::env::var("ENV_FILE").expect("environment configuration file to exist");
    let config = HashMap::from_iter([("env".to_string(), env_file)]);
    let vault_direct = kv_vault_lib::client::Client::new(
        kv_vault_lib::config::Config::from_values(&config).expect("configuration to be valid"),
    )
    .expect("client from defaults");

    // test pulling data when other processes have saved json data
    let mut map1 = HashMap::new();
    map1.insert("foo".to_string(), "bar".to_string());
    vault_direct
        .write_secret("test_map_foo", &map1)
        .await
        .expect("write map1");

    let mut map2 = HashMap::new();
    map2.insert(STRING_VALUE_MARKER.to_string(), "42".to_string());
    vault_direct
        .write_secret("test_map_data", &map2)
        .await
        .expect("write map2");

    // create client and ctx
    let kv = KeyValueSender::via(prov);
    let ctx = Context::default();

    let get_map1 = kv.get(&ctx, "test_map_foo").await.expect("get map1");
    check!(get_map1.exists).expect("get map1 exists");
    check_eq!(get_map1.value.as_str(), r#"{"foo":"bar"}"#).expect("expected json map");

    let get_map2 = kv.get(&ctx, "test_map_data").await.expect("get map2");
    check!(get_map2.exists).expect("get map2");
    check_eq!(get_map2.value.as_str(), "42").expect("map2 expect data value 42");

    // clean up
    kv.del(&ctx, "test_map_foo")
        .await
        .expect("delete test_map_foo");
    kv.del(&ctx, "test_map_data")
        .await
        .expect("delete test_map_data");

    Ok(())
}

/// tests renewal of token
async fn renewal(_opt: &TestOptions) -> RpcResult<()> {
    let token = std::env::var("SHORT_LIVED_TOKEN")
        .expect("token to exist in env. Run this test with `run-test.sh`.");

    // Slightly annoying parsing of env file and then overwriting the VAULT_TOKEN with the short lived token
    let env_file = std::env::var("ENV_FILE").expect("environment configuration file to exist");
    let data = std::fs::read_to_string(env_file).expect("env file to load properly");
    simple_env_load::parse_and_set(&data, |k, v| std::env::set_var(k, v));
    std::env::set_var("VAULT_TOKEN", token.to_string());

    let config_values = std::collections::HashMap::from_iter([
        ("token_increment_ttl".to_string(), "130s".to_string()),
        ("token_refresh_interval".to_string(), "5".to_string()),
    ]);
    let config = kv_vault_lib::config::Config::from_values(&config_values)
        .expect("configuration to be valid");
    let vault_direct = kv_vault_lib::client::Client::new(config).expect("client from defaults");
    vault_direct.set_renewal().await;

    let current_token_info = vaultrs::token::lookup(vault_direct.inner_client().as_ref(), &token)
        .await
        .expect("token to exist");
    // Should have enough time to carry out the rest of the test
    assert!(current_token_info.ttl > 10);

    // Allow the token to renew, should have an additional duration of 60 seconds afterwards
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let secret_path = "secret/data/veryverysecret";
    let value: Value = serde_json::from_str("ALICE").unwrap_or_else(|_| {
        let mut map = serde_json::Map::new();
        map.insert(
            STRING_VALUE_MARKER.to_string(),
            Value::String("ALICE".to_string()),
        );
        Value::Object(map)
    });
    let _put = vault_direct
        .write_secret(secret_path, &value)
        .await
        .expect("should be able to write secret");

    // Wait an entire renewal period to ensure the token has expired and renews
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let secret: serde_json::Value = vault_direct
        .read_secret(secret_path)
        .await
        .expect("should be able to read secret");

    assert_eq!(secret, value);

    let renewed_token_info = vaultrs::token::lookup(vault_direct.inner_client().as_ref(), &token)
        .await
        .expect("token to exist");

    // Once the renewal loop starts, the ttl should _always_ be greater than what we looked up when
    // we started the client with current token. Subtract 5 seconds to account for any weird edge
    // cases where we got the "current" ttl right after being renewed.
    //
    // Put another way, if this check fails it means the token was never renewed.
    assert!(renewed_token_info.ttl >= current_token_info.ttl - 5);
    // We also renew with a ttl of 130 which is above the initial ttl of 120, so we can
    // assert that too.
    assert!(renewed_token_info.ttl > 120);

    Ok(())
}
