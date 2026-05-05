use anyhow::{Context, Result, bail};
use std::env;
use tokio::io::AsyncReadExt;
use url::Url;

pub async fn fetch(output: &str) -> Result<Vec<u8>> {
    let url = Url::parse(output).with_context(|| format!("invalid precompiled URL: {output}"))?;
    match url.scheme() {
        "nats" => fetch_nats(&url).await,
        "file" => fetch_file(&url),
        other => bail!("unsupported precompiled scheme: {other}"),
    }
}

fn fetch_file(url: &Url) -> Result<Vec<u8>> {
    let path = url
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("invalid file:// URL: {url}"))?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("failed to read precompiled bytes from {}", path.display()))?;
    Ok(bytes)
}

async fn fetch_nats(url: &Url) -> Result<Vec<u8>> {
    let (bucket, key) = parse_nats_url(url)?;

    let nats_url = env::var("NATS_URL").context("NATS_URL env var not set")?;
    let client = async_nats::connect(&nats_url)
        .await
        .with_context(|| format!("failed to connect to NATS at {nats_url}"))?;
    let jetstream = async_nats::jetstream::new(client);
    let store = jetstream
        .get_object_store(&bucket)
        .await
        .with_context(|| format!("object store '{bucket}' not found"))?;

    let mut object = store
        .get(key.as_str())
        .await
        .with_context(|| format!("object '{key}' not found in '{bucket}'"))?;

    let mut bytes = Vec::new();
    object
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("failed to read object '{key}'"))?;

    Ok(bytes)
}

fn parse_nats_url(url: &Url) -> Result<(String, String)> {
    let bucket = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("nats:// URL missing bucket: {url}"))?
        .to_string();
    let key = url.path().trim_start_matches('/').to_string();
    if key.is_empty() {
        bail!("nats:// URL missing object key: {url}");
    }
    Ok((bucket, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nats_url_into_bucket_and_key() {
        let url = Url::parse("nats://precompiled-artifacts/myapp/x86_64.cwasm").unwrap();
        let (bucket, key) = parse_nats_url(&url).unwrap();
        assert_eq!(bucket, "precompiled-artifacts");
        assert_eq!(key, "myapp/x86_64.cwasm");
    }

    #[test]
    fn nats_url_without_key_errors() {
        let url = Url::parse("nats://bucket/").unwrap();
        let err = parse_nats_url(&url).unwrap_err();
        assert!(err.to_string().contains("missing object key"));
    }

    #[tokio::test]
    async fn fetches_bytes_from_file_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.cwasm");
        std::fs::write(&path, b"hello").unwrap();
        let url = format!("file://{}", path.display());

        let bytes = fetch(&url).await.unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn unknown_scheme_errors() {
        let err = fetch("s3://bucket/key").await.unwrap_err();
        assert!(err.to_string().contains("unsupported precompiled scheme"));
    }
}
