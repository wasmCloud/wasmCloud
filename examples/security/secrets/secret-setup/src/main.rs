use secrets_nats_kv::put_secrets;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // put secrets in secret store
    let component_secret = wasmcloud_secrets_types::Secret {
        name: "api_password".to_string(),
        string_secret: Some("opensesame".to_string()),
        ..Default::default()
    };
    let provider_secret = wasmcloud_secrets_types::Secret {
        name: "redis_password".to_string(),
        string_secret: Some("sup3rS3cr3tP4ssw0rd".to_string()),
        ..Default::default()
    };
    let default_provider_secret = wasmcloud_secrets_types::Secret {
        name: "default_redis_password".to_string(),
        string_secret: Some("sup3rS3cr3tP4ssw0rd".to_string()),
        ..Default::default()
    };

    let nats_client = async_nats::connect("127.0.0.1:4222").await?;
    let transit_xkey_seed = std::env::args()
        .nth(1)
        .expect("transit xkey required to put secrets");
    let transit_xkey =
        nkeys::XKey::from_seed(&transit_xkey_seed).expect("transit xkey seed is invalid");

    let results = put_secrets(
        &nats_client,
        "wasmcloud.secrets",
        &transit_xkey,
        vec![component_secret, provider_secret, default_provider_secret],
    )
    .await;
    for res in results {
        res?;
    }

    Ok(())
}
