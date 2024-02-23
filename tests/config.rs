use std::collections::HashMap;

use anyhow::{Context, Result};

use async_nats::jetstream;
use wasmcloud_host::wasmbus::config::BundleGenerator;

pub mod common;
use common::nats::start_nats;

#[tokio::test(flavor = "multi_thread")]
async fn config_updates() -> Result<()> {
    let (nats_server, _, nats_client) = start_nats()
        .await
        .context("failed to start backing services")?;

    let store = jetstream::new(nats_client)
        .create_key_value(jetstream::kv::Config {
            bucket: "CONFIG".into(),
            ..Default::default()
        })
        .await
        .context("Unable to set up NATS KV store for test")?;

    let generator = BundleGenerator::new(store.clone());

    // First test that a non-existent config item returns an error
    generator
        .generate(vec!["inoexist".to_string()])
        .await
        .map(|_| ())
        .expect_err("Should have errored if a config didn't exist");

    // Now create some config items
    put_config(&store, "foo", [("star".to_string(), "wars".to_string())]).await?;
    put_config(
        &store,
        "bar",
        [("captain".to_string(), "picard".to_string())],
    )
    .await?;

    let bundle = generator
        .generate(vec!["foo".to_string(), "bar".to_string()])
        .await
        .expect("Should be able to generate config bundle");
    // Give it a sec to populate from the store
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        *bundle.get_config().await,
        HashMap::from([
            ("star".to_string(), "wars".to_string()),
            ("captain".to_string(), "picard".to_string())
        ])
    );

    // Update the config, give it a sec to update and then make sure it updated properly
    put_config(
        &store,
        "bar",
        [
            ("captain".to_string(), "picard".to_string()),
            ("star".to_string(), "trek".to_string()),
        ],
    )
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        *bundle.get_config().await,
        HashMap::from([
            ("star".to_string(), "trek".to_string()),
            ("captain".to_string(), "picard".to_string())
        ])
    );

    // Generate a new bundle using the foo config and make sure it has the right data. This also should exercise the caching behavior, though it is hard to actually test it is pulling from the cache
    let bundle2 = generator
        .generate(vec!["foo".to_string()])
        .await
        .expect("Should be able to generate config bundle");
    assert_eq!(
        *bundle2.get_config().await,
        HashMap::from([("star".to_string(), "wars".to_string())])
    );

    let _ = nats_server.stop().await;
    Ok(())
}

async fn put_config(
    store: &jetstream::kv::Store,
    name: &str,
    config: impl Into<HashMap<String, String>>,
) -> Result<()> {
    let data = serde_json::to_vec(&config.into()).expect("Should be able to serialize config");
    store
        .put(name, data.into())
        .await
        .context("Failed to put config")
        .map(|_| ())
}
