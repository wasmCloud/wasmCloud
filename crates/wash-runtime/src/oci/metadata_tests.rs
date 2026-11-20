//! Metadata limits exercised through the registry client's HTTP transport.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const CHILD_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone, Copy, Debug)]
enum Target {
    Manifest,
    Index,
    ChildManifest,
    Token,
    ManifestError,
    TokenError,
    TokenAtBlobPath,
    BlobError,
    Blob,
    BlobRedirect,
    BlobAlternate,
    DigestFallback,
    ValidManifest,
}

struct Registry {
    reference: Reference,
    sent: Arc<AtomicUsize>,
    tasks: tokio::task::JoinHandle<()>,
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.tasks.abort();
    }
}

impl Registry {
    async fn start(target: Target, declared_size: Option<usize>, payload_size: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let sent = Arc::new(AtomicUsize::new(0));
        let tasks = tokio::spawn({
            let sent = sent.clone();
            async move {
                let mut connections = tokio::task::JoinSet::new();
                while let Ok((stream, _)) = listener.accept().await {
                    let sent = sent.clone();
                    connections.spawn(async move {
                        // Closing a refused HTTP response normally breaks its
                        // server-side write; only the client result is asserted.
                        let _ =
                            serve(stream, address, target, declared_size, payload_size, sent).await;
                    });
                }
            }
        });
        Self {
            reference: format!("{address}/test/component:latest").parse().unwrap(),
            sent,
            tasks,
        }
    }

    fn client(&self) -> Client {
        crate::init_crypto();
        oci_client(ClientConfig {
            protocol: ClientProtocol::Http,
            platform_resolver: Some(Box::new(|_| Some(CHILD_DIGEST.to_owned()))),
            ..Default::default()
        })
        .unwrap()
    }
}

async fn respond(stream: &mut TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status} response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(body).await
}

async fn serve(
    mut stream: TcpStream,
    address: std::net::SocketAddr,
    target: Target,
    declared_size: Option<usize>,
    payload_size: usize,
    sent: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut byte = [0];
    while request.len() < 16 * 1024 && !request.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await? == 0 {
            return Ok(());
        }
        request.push(byte[0]);
    }
    let request = String::from_utf8_lossy(&request);
    let path = request.split_whitespace().nth(1).unwrap();
    if request.starts_with("HEAD ") {
        return respond(&mut stream, 200, b"").await;
    }
    if path == "/v2/" {
        if matches!(
            target,
            Target::Token | Target::TokenError | Target::TokenAtBlobPath
        ) {
            let realm = if matches!(target, Target::TokenAtBlobPath) {
                format!("/v2/test/component/blobs/{CHILD_DIGEST}")
            } else {
                "/token".to_owned()
            };
            return stream
                .write_all(format!("HTTP/1.1 401 response\r\nWWW-Authenticate: Bearer realm=\"http://{address}{realm}\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
                .await;
        }
        return respond(&mut stream, 200, b"{}").await;
    }
    if matches!(target, Target::TokenAtBlobPath) && request.contains("\r\naccept:") {
        // The OCI client can continue anonymously after a token error. End
        // that blob request so only an unbounded token read would hang here.
        return respond(&mut stream, 401, b"{}").await;
    }
    if matches!(
        target,
        Target::Blob | Target::BlobRedirect | Target::BlobAlternate
    ) {
        if matches!(target, Target::BlobRedirect) && path != "/download" {
            return stream.write_all(format!("HTTP/1.1 307 redirect\r\nLocation: http://{address}/download\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await;
        }
        if matches!(target, Target::BlobAlternate) && path != "/download" {
            return respond(&mut stream, 404, b"{}").await;
        }
        return respond(&mut stream, 200, &vec![b'x'; payload_size]).await;
    }
    if matches!(target, Target::ValidManifest) {
        let mut manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.wasm.config.v0+json",
                "digest": CHILD_DIGEST,
                "size": 0,
            },
            "layers": [],
        }))
        .unwrap();
        manifest.resize(payload_size, b' ');
        if declared_size.is_none() {
            stream
                .write_all(b"HTTP/1.1 200 response\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                .await?;
            stream
                .write_all(format!("{:x}\r\n", manifest.len()).as_bytes())
                .await?;
            stream.write_all(&manifest).await?;
            return stream.write_all(b"\r\n0\r\n\r\n").await;
        }
        return respond(&mut stream, 200, &manifest).await;
    }
    if matches!(target, Target::ChildManifest) && path.ends_with("/latest") {
        let index = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [{
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": CHILD_DIGEST,
                "size": payload_size,
            }],
        }))
        .unwrap();
        return respond(&mut stream, 200, &index).await;
    }
    let status = if matches!(
        target,
        Target::ManifestError | Target::TokenError | Target::BlobError
    ) {
        500
    } else {
        200
    };
    let mime = if matches!(target, Target::Index) {
        "application/vnd.oci.image.index.v1+json"
    } else {
        "application/vnd.oci.image.manifest.v1+json"
    };
    let framing = declared_size.map_or_else(
        || "Transfer-Encoding: chunked".to_owned(),
        |size| format!("Content-Length: {size}"),
    );
    stream
        .write_all(format!("HTTP/1.1 {status} response\r\nContent-Type: {mime}\r\n{framing}\r\nConnection: close\r\n\r\n").as_bytes())
        .await?;

    // Stop without an EOF or final chunk. A buffering client would hang here;
    // the bounded client must fail as soon as it sees limit + 1 bytes.
    let chunk = [b' '; 16 * 1024];
    let mut remaining = payload_size;
    while remaining > 0 {
        let len = remaining.min(chunk.len());
        if declared_size.is_none() {
            stream.write_all(format!("{len:x}\r\n").as_bytes()).await?;
        }
        stream.write_all(&chunk[..len]).await?;
        if declared_size.is_none() {
            stream.write_all(b"\r\n").await?;
        }
        sent.fetch_add(len, Ordering::SeqCst);
        remaining -= len;
    }
    let _ = stream.read(&mut byte).await?;
    Ok(())
}

#[tokio::test]
async fn chunked_metadata_is_refused_without_waiting_for_the_response_to_end() {
    let limit = transport::MAX_METADATA_RESPONSE_SIZE;
    for target in [
        Target::Manifest,
        Target::Index,
        Target::ChildManifest,
        Target::Token,
        Target::ManifestError,
        Target::TokenError,
        Target::BlobError,
        Target::DigestFallback,
    ] {
        let registry = Registry::start(target, None, limit + 1).await;
        let client = registry.client();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            if matches!(target, Target::DigestFallback) {
                client
                    .fetch_manifest_digest(&registry.reference, &RegistryAuth::Anonymous)
                    .await
                    .err()
            } else if matches!(target, Target::BlobError) {
                client
                    .pull_blob_stream_partial(&registry.reference, CHILD_DIGEST, 0, None)
                    .await
                    .err()
            } else {
                client
                    .pull_image_manifest(&registry.reference, &RegistryAuth::Anonymous)
                    .await
                    .err()
            }
        })
        .await
        .expect("the response must be refused before its withheld tail arrives");
        let error = result.expect("oversized metadata must fail");
        assert!(
            error
                .to_string()
                .contains("registry metadata response exceeds"),
            "{target:?}: {error}"
        );
    }
}

#[tokio::test]
async fn metadata_at_the_byte_ceiling_remains_usable() {
    let limit = transport::MAX_METADATA_RESPONSE_SIZE;
    for declared_size in [Some(limit), None] {
        let registry = Registry::start(Target::ValidManifest, declared_size, limit).await;
        let (manifest, _) = registry
            .client()
            .pull_image_manifest(&registry.reference, &RegistryAuth::Anonymous)
            .await
            .unwrap();
        assert_eq!(manifest.schema_version, 2);
    }
}

#[tokio::test]
async fn declared_oversized_metadata_is_refused_without_reading_the_body() {
    let limit = transport::MAX_METADATA_RESPONSE_SIZE;
    let registry = Registry::start(Target::Manifest, Some(limit + 1), 0).await;
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        registry
            .client()
            .pull_image_manifest(&registry.reference, &RegistryAuth::Anonymous),
    )
    .await
    .expect("the body is withheld")
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("registry metadata response exceeds")
    );
    assert_eq!(registry.sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn blob_operations_still_bound_authentication_and_error_responses() {
    for target in [Target::TokenAtBlobPath, Target::BlobError] {
        let registry =
            Registry::start(target, None, transport::MAX_METADATA_RESPONSE_SIZE + 1).await;
        let layer = OciDescriptor {
            digest: CHILD_DIGEST.to_owned(),
            ..Default::default()
        };
        let client = registry.client();
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            transport::pull_blob(&client, &registry.reference, &layer, tokio::io::sink()),
        )
        .await
        .expect("metadata must be refused before its withheld tail")
        .unwrap_err();
        if matches!(target, Target::BlobError) {
            assert!(format!("{error:#}").contains("registry metadata response exceeds"));
        }
    }
}

#[tokio::test]
async fn blob_downloads_can_stream_beyond_the_metadata_limit() {
    let size = transport::MAX_METADATA_RESPONSE_SIZE + 1;
    let digest = format!("sha256:{:x}", Sha256::digest(vec![b'x'; size]));
    for target in [Target::Blob, Target::BlobRedirect, Target::BlobAlternate] {
        let registry = Registry::start(target, None, size).await;
        let layer = OciDescriptor {
            digest: digest.clone(),
            size: size as i64,
            urls: Some(vec![format!(
                "http://{}/download",
                registry.reference.registry()
            )]),
            ..Default::default()
        };
        let client = registry.client();
        tokio::time::timeout(
            Duration::from_secs(10),
            transport::pull_blob(&client, &registry.reference, &layer, tokio::io::sink()),
        )
        .await
        .unwrap()
        .unwrap();
    }
}

#[tokio::test]
async fn a_concurrent_blob_download_cannot_relax_manifest_limits() {
    let size = transport::MAX_METADATA_RESPONSE_SIZE + 1;
    let registry = Registry::start(Target::Blob, None, size).await;
    let client = registry.client();
    let layer = OciDescriptor {
        digest: format!("sha256:{:x}", Sha256::digest(vec![b'x'; size])),
        size: size as i64,
        ..Default::default()
    };
    let (blob, manifest) = tokio::join!(
        transport::pull_blob(&client, &registry.reference, &layer, tokio::io::sink()),
        client.pull_image_manifest(&registry.reference, &RegistryAuth::Anonymous),
    );
    blob.unwrap();
    assert!(
        manifest
            .unwrap_err()
            .to_string()
            .contains("registry metadata response exceeds")
    );
}
