use std::env::consts::{ARCH, OS};

use anyhow::{anyhow, Context as _};
use nkeys::KeyPair;
use once_cell::sync::Lazy;
use provider_archive::ProviderArchive;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::sync::OnceCell;
use url::Url;

static ISSUER: Lazy<KeyPair> = Lazy::new(KeyPair::new_account);

pub struct Provider {
    pub par: NamedTempFile,
    pub subject: KeyPair,
}

impl Provider {
    pub async fn new(capid: &str, name: &str, bin: &str) -> anyhow::Result<Self> {
        let mut par = ProviderArchive::new(capid, name, "test", None, None);
        let bin = fs::read(bin)
            .await
            .with_context(|| format!("failed to read binary at `{bin}`"))?;
        par.add_library(&format!("{ARCH}-{OS}"), &bin)
            .map_err(|e| anyhow!(e).context("failed to add  binary to PAR"))?;
        let subject = KeyPair::new_service();
        let tmp = NamedTempFile::new().context("failed to create temporary file")?;
        par.write(tmp.path(), &ISSUER, &subject, false)
            .await
            .map_err(|e| anyhow!(e).context("failed to write PAR"))?;
        Ok(Self { par: tmp, subject })
    }

    pub fn url(&self) -> Url {
        Url::from_file_path(self.par.path()).expect("failed to construct URL to PAR")
    }
}

static RUST_BLOBSTORE_FS: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_blobstore_fs() -> &'static Provider {
    RUST_BLOBSTORE_FS
        .get_or_init(|| async {
            Provider::new(
                "wrpc:blobstore",
                "wasmcloud-provider-blobstore-fs",
                env!("CARGO_BIN_EXE_blobstore-fs"),
            )
            .await
            .expect("failed to build blobstore-fs PAR")
        })
        .await
}

static RUST_BLOBSTORE_S3: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_blobstore_s3() -> &'static Provider {
    RUST_BLOBSTORE_S3
        .get_or_init(|| async {
            Provider::new(
                "wrpc:blobstore",
                "wasmcloud-provider-blobstore-s3",
                env!("CARGO_BIN_EXE_blobstore-s3"),
            )
            .await
            .expect("failed to build blobstore-s3 PAR")
        })
        .await
}

static RUST_HTTP_CLIENT: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_http_client() -> &'static Provider {
    RUST_HTTP_CLIENT
        .get_or_init(|| async {
            Provider::new(
                "wrpc:http/outgoing-handler",
                "wasmcloud-provider-http-client",
                env!("CARGO_BIN_EXE_http-client"),
            )
            .await
            .expect("failed to build http-client PAR")
        })
        .await
}

static RUST_HTTP_SERVER: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_http_server() -> &'static Provider {
    RUST_HTTP_SERVER
        .get_or_init(|| async {
            Provider::new(
                "wrpc:http/incoming-handler",
                "wasmcloud-provider-http-server",
                env!("CARGO_BIN_EXE_http-server"),
            )
            .await
            .expect("failed to build http-server PAR")
        })
        .await
}

static RUST_LATTICE_CONTROLLER: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_lattice_controller() -> &'static Provider {
    RUST_LATTICE_CONTROLLER
        .get_or_init(|| async {
            Provider::new(
                "wasmcloud:latticecontrol",
                "wasmcloud-provider-lattice-controller",
                env!("CARGO_BIN_EXE_lattice-controller"),
            )
            .await
            .expect("failed to build lattice-controller PAR")
        })
        .await
}

static RUST_KEYVALUE_REDIS: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_keyvalue_redis() -> &'static Provider {
    RUST_KEYVALUE_REDIS
        .get_or_init(|| async {
            Provider::new(
                "wrpc:keyvalue",
                "wasmcloud-provider-keyvalue-redis",
                env!("CARGO_BIN_EXE_keyvalue-redis"),
            )
            .await
            .expect("failed to build keyvalue-redis PAR")
        })
        .await
}

static RUST_KEYVALUE_VAULT: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_keyvalue_vault() -> &'static Provider {
    RUST_KEYVALUE_VAULT
        .get_or_init(|| async {
            Provider::new(
                "wrpc:keyvalue",
                "wasmcloud-provider-keyvalue-vault",
                env!("CARGO_BIN_EXE_keyvalue-vault"),
            )
            .await
            .expect("failed to build keyvalue-vault PAR")
        })
        .await
}

static RUST_MESSAGING_KAFKA: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_messaging_kafka() -> &'static Provider {
    RUST_MESSAGING_KAFKA
        .get_or_init(|| async {
            Provider::new(
                "wasmcloud:messaging",
                "wasmcloud-provider-messaging-kafka",
                env!("CARGO_BIN_EXE_messaging-kafka"),
            )
            .await
            .expect("failed to build messaging-kafka PAR")
        })
        .await
}

static RUST_MESSAGING_NATS: OnceCell<Provider> = OnceCell::const_new();
pub async fn rust_messaging_nats() -> &'static Provider {
    RUST_MESSAGING_NATS
        .get_or_init(|| async {
            Provider::new(
                "wasmcloud:messaging",
                "wasmcloud-provider-messaging-nats",
                env!("CARGO_BIN_EXE_messaging-nats"),
            )
            .await
            .expect("failed to build messaging-nats PAR")
        })
        .await
}
