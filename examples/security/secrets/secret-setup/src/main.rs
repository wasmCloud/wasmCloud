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

    let nats_client = async_nats::connect("127.0.0.1:4222").await?;

    put_secret(&nats_client, component_secret).await?;
    put_secret(&nats_client, provider_secret).await?;

    Ok(())
}

async fn put_secret(
    nats_client: &async_nats::Client,
    secret: wasmcloud_secrets_types::Secret,
) -> Result<(), Box<dyn std::error::Error>> {
    let request_xkey = nkeys::XKey::new();
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(
        wasmcloud_secrets_types::WASMCLOUD_HOST_XKEY,
        request_xkey
            .public_key()
            .parse::<async_nats::HeaderValue>()
            .unwrap(),
    );

    let xkey = std::env::args().nth(1).expect("xkey to be passed");
    let transit_xkey = nkeys::XKey::from_seed(&xkey).expect("transit xkey to be valid");
    let transit_xkey_pub = nkeys::XKey::from_public_key(&transit_xkey.public_key())
        .expect("transit public key to be valid");
    let value = serde_json::to_string(&secret)?;
    let v = request_xkey
        .seal(value.as_bytes(), &transit_xkey_pub)
        .expect("should be able to encrypt the secret");
    nats_client
        .request_with_headers("wasmcloud.secrets.v0.nats-kv.put_secret", headers, v.into())
        .await?;

    Ok(())
}
