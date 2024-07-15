use std::collections::HashSet;

use async_nats::{jetstream, Client};
use nkeys::{KeyPair, XKey};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use secrets_nats_kv::{Api, PutSecretResponse};
use std::collections::HashMap;
use wascap::jwt::{Claims, ClaimsBuilder, Component, Host};
use wasmcloud_secrets_types::{Context, Secret, SecretRequest, WASMCLOUD_HOST_XKEY};

const SUBJECT_BASE: &str = "kvstore_test";
const NAME_BASE: &str = "nats-kv";
const TEST_API_VERSION: &str = "test";

struct Suite {
    name: String,
}

impl Drop for Suite {
    fn drop(&mut self) {
        let name = self.name.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
                let js = jetstream::new(client.clone());
                js.delete_key_value(format!("SECRETS_{}_state", name.clone()))
                    .await
                    .unwrap();
                js.delete_key_value(name.clone()).await.unwrap();
                js.delete_stream(format!("SECRETS_{}_state_lock", name.clone()))
                    .await
                    .unwrap();
            });
        })
        .join()
        .unwrap();
    }
}

#[tokio::test]
async fn integration_test_kvstore_basic() -> anyhow::Result<()> {
    let client = async_nats::connect("127.0.0.1:4222").await?;

    let encryption_xkey = XKey::new();
    let server_xkey = XKey::new();
    let server_pub = server_xkey.public_key();
    let (api, name) = setup_api(
        client.clone(),
        encryption_xkey.seed().unwrap(),
        server_xkey.seed().unwrap(),
    );

    let base_sub = api.subject();
    let _suite = Suite { name: name.clone() };
    tokio::spawn(async move {
        api.run().await.unwrap();
    });

    // Give the server some time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let resp = client
        .request(format!("{base_sub}.server_xkey"), "".into())
        .await?;
    println!("{:?}", resp);
    let payload = resp.payload;
    let s = std::str::from_utf8(&payload).unwrap();
    let key = XKey::from_public_key(s).unwrap();
    assert_eq!(key.public_key(), server_pub);

    Ok(())
}

#[tokio::test]
async fn integration_test_kvstore_put_secret() -> anyhow::Result<()> {
    let client = async_nats::connect("127.0.0.1:4222").await?;

    let encryption_xkey = XKey::new();
    let server_xkey = XKey::new();
    let request_key = XKey::new();

    let (api, name) = setup_api(
        client.clone(),
        encryption_xkey.seed().unwrap(),
        server_xkey.seed().unwrap(),
    );

    let base_sub = api.subject();
    let _suite = Suite { name: name.clone() };
    tokio::spawn(async move {
        api.run().await.unwrap();
    });
    // Give the server some time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let value = Secret {
        name: "test".to_string(),
        string_secret: Some("value".to_string()),
        ..Default::default()
    };

    let value = serde_json::to_string(&value).unwrap();
    let v = request_key.seal(value.as_bytes(), &server_xkey).unwrap();

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(WASMCLOUD_HOST_XKEY, request_key.public_key().as_str());

    let resp = client
        .request_with_headers(format!("{base_sub}.put_secret"), headers.clone(), v.into())
        .await?;
    let payload = resp.payload;
    let revision: PutSecretResponse = serde_json::from_slice(&payload).unwrap();
    assert_eq!(revision.revision, 1);

    // TODO remove this once wasmcloud uses the latest version of nkeys
    let account = wascap::prelude::KeyPair::new_account();
    let component_key = KeyPair::new_module();
    let claims: Claims<Component> = ClaimsBuilder::new()
        .issuer(account.public_key().as_str())
        .subject(component_key.public_key().as_str())
        .build();

    let encoded = claims.encode(&account)?;
    let mut v: HashSet<String> = HashSet::new();
    v.insert("test".to_string());

    let payload = serde_json::to_string(&v).unwrap();
    let response = client
        .request(
            format!("{base_sub}.add_mapping.{}", component_key.public_key()),
            payload.into(),
        )
        .await?;
    println!("{:?}", response);
    assert_eq!(response.payload.to_vec(), b"ok");

    let host_key = KeyPair::new_server();
    let claims: Claims<Host> = ClaimsBuilder::new()
        .issuer(account.public_key().as_str())
        .subject(host_key.public_key().as_str())
        .with_metadata(Host::new("test".to_string(), HashMap::new()))
        .build();

    let request = SecretRequest {
        name: "test".to_string(),
        context: Context {
            entity_jwt: encoded,
            host_jwt: claims.encode(&account)?,
            application: None,
        },
        version: None,
    };
    let nats_client = async_nats::connect("127.0.0.1:4222").await?;
    let secrets_client = wasmcloud_secrets_client::Client::new_with_version(
        &name,
        SUBJECT_BASE,
        nats_client,
        Some(TEST_API_VERSION),
    )
    .await?;

    let resp = secrets_client.get(request, request_key).await?;
    assert_eq!(resp.string_secret.unwrap(), "value");

    Ok(())
}

#[tokio::test]
async fn integration_test_kvstore_version() -> anyhow::Result<()> {
    let client = async_nats::connect("127.0.0.1:4222").await?;

    let encryption_xkey = XKey::new();
    let server_xkey = XKey::new();
    let request_key = XKey::new();

    let (api, name) = setup_api(
        client.clone(),
        encryption_xkey.seed().unwrap(),
        server_xkey.seed().unwrap(),
    );

    let base_sub = api.subject();
    let _suite = Suite { name: name.clone() };
    tokio::spawn(async move {
        api.run().await.unwrap();
    });
    // Give the server some time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let value = Secret {
        name: "test".to_string(),
        string_secret: Some("value".to_string()),
        ..Default::default()
    };

    let value = serde_json::to_string(&value).unwrap();
    let v = request_key.seal(value.as_bytes(), &server_xkey).unwrap();

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(WASMCLOUD_HOST_XKEY, request_key.public_key().as_str());

    let resp = client
        .request_with_headers(
            format!("{base_sub}.put_secret"),
            headers.clone(),
            v.clone().into(),
        )
        .await?;
    let payload = resp.payload;
    let revision: PutSecretResponse = serde_json::from_slice(&payload).unwrap();
    assert_eq!(revision.revision, 1);

    let resp = client
        .request_with_headers(format!("{base_sub}.put_secret"), headers.clone(), v.into())
        .await?;
    let payload = resp.payload;
    let revision: PutSecretResponse = serde_json::from_slice(&payload).unwrap();
    assert_eq!(revision.revision, 2);

    // TODO remove this once wasmcloud uses the latest version of nkeys
    let account = wascap::prelude::KeyPair::new_account();
    let component_key = KeyPair::new_module();
    let claims: Claims<Component> = ClaimsBuilder::new()
        .issuer(account.public_key().as_str())
        .subject(component_key.public_key().as_str())
        .build();

    let encoded = claims.encode(&account)?;
    let mut v: HashSet<String> = HashSet::new();
    v.insert("test".to_string());

    let payload = serde_json::to_string(&v).unwrap();
    let response = client
        .request(
            format!("{base_sub}.add_mapping.{}", component_key.public_key()),
            payload.into(),
        )
        .await?;
    println!("{:?}", response);
    assert_eq!(response.payload.to_vec(), b"ok");

    let host_key = KeyPair::new_server();
    let claims: Claims<Host> = ClaimsBuilder::new()
        .issuer(account.public_key().as_str())
        .subject(host_key.public_key().as_str())
        .with_metadata(Host::new("test".to_string(), HashMap::new()))
        .build();

    let request = SecretRequest {
        name: "test".to_string(),
        context: Context {
            entity_jwt: encoded,
            host_jwt: claims.encode(&account)?,
            application: None,
        },
        version: Some("1".to_string()),
    };

    let nats_client = async_nats::connect("127.0.0.1:4222").await?;
    let secrets_client = wasmcloud_secrets_client::Client::new_with_version(
        &name,
        SUBJECT_BASE,
        nats_client,
        Some(TEST_API_VERSION),
    )
    .await?;
    let resp = secrets_client.get(request, request_key).await?;

    assert_eq!(resp.string_secret.unwrap(), "value");
    assert_eq!(resp.version, "1");

    Ok(())
}

fn setup_api(client: Client, enc_seed: String, server_seed: String) -> (Api, String) {
    let server_xkey = XKey::from_seed(&server_seed).unwrap();
    let encryption_key = XKey::from_seed(&enc_seed).unwrap();

    let suffix = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect::<String>();
    let name = format!("{}-{}", NAME_BASE, suffix);

    (
        Api::new(
            server_xkey,
            encryption_key,
            client.clone(),
            SUBJECT_BASE.to_string(),
            name.clone(),
            name.clone(),
            64,
            "wasmcloud_secrets_test".to_string(),
            TEST_API_VERSION.to_string(),
        ),
        name,
    )
}
