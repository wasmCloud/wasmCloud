#![allow(clippy::type_complexity)]

//! blobstore-s3 capability provider
//!
//! This capability provider exposes [S3](https://aws.amazon.com/s3/)-compatible object storage
//! (AKA "blob store") as a [wasmcloud capability](https://wasmcloud.com/docs/concepts/capabilities) which
//! can be used by actors on your lattice.
//!

use core::future::Future;
use core::pin::Pin;
use core::str::FromStr;

use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::default_provider::region::DefaultRegionChain;
use aws_config::retry::RetryConfig;
use aws_config::sts::AssumeRoleProvider;
use aws_sdk_s3::config::{Region, SharedCredentialsProvider};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::operation::create_bucket::{CreateBucketError, CreateBucketOutput};
use aws_sdk_s3::operation::get_object::GetObjectOutput;
use aws_sdk_s3::operation::head_bucket::HeadBucketError;
use aws_sdk_s3::operation::head_object::{HeadObjectError, HeadObjectOutput};
use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Output;
use aws_sdk_s3::types::{
    BucketLocationConstraint, CreateBucketConfiguration, Delete, Object, ObjectIdentifier,
};
use aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder;
use base64::Engine as _;
use bindings::wrpc::blobstore::types::{ContainerMetadata, ObjectId, ObjectMetadata};
use bytes::{Bytes, BytesMut};
use futures::{stream, Stream, StreamExt as _};
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, propagate_trace_for_ctx, run_provider,
    serve_provider_exports, Context, LinkConfig, LinkDeleteInfo, Provider,
};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:io/error@0.2.0": generate,
            "wasi:io/poll@0.2.0": generate,
            "wasi:io/streams@0.2.0": generate,
            "wrpc:blobstore/blobstore@0.2.0": generate,
            "wrpc:blobstore/types@0.2.0": generate,
        }
    });
}

const ALIAS_PREFIX: &str = "alias_";
const DEFAULT_STS_SESSION: &str = "blobstore_s3_provider";

/// Configuration for connecting to S3-compatible storage
///
/// This value is meant to be parsed from link configuration, and can
/// represent any S3-compatible storage (excluding AWS-specific things like STS)
///
/// NOTE that when storage config is provided via link configuration
#[derive(Clone, Debug, Default, Deserialize)]
pub struct StorageConfig {
    /// AWS_ACCESS_KEY_ID, can be specified from environment
    pub access_key_id: Option<String>,
    /// AWS_SECRET_ACCESS_KEY, can be in environment
    pub secret_access_key: Option<String>,
    /// Session Token
    pub session_token: Option<String>,
    /// AWS_REGION
    pub region: Option<String>,
    /// override default max_attempts (3) for retries
    pub max_attempts: Option<u32>,
    /// optional configuration for STS Assume Role
    pub sts_config: Option<StsAssumeRoleConfig>,
    /// optional override for the AWS endpoint
    pub endpoint: Option<String>,
    /// optional map of bucket aliases to names
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    /// Region in which buckets will be created
    pub bucket_region: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct StsAssumeRoleConfig {
    /// Role to assume (AWS_ASSUME_ROLE_ARN)
    /// Should be in the form "arn:aws:iam::123456789012:role/example"
    pub role: String,
    /// AWS Region for using sts, not for S3
    pub region: Option<String>,
    /// Optional Session name
    pub session: Option<String>,
    /// Optional external id
    pub external_id: Option<String>,
}

impl StorageConfig {
    /// initialize from linkdef values
    pub async fn from_link_config(
        LinkConfig {
            config, secrets, ..
        }: &LinkConfig<'_>,
    ) -> Result<StorageConfig> {
        let mut storage_config = if let Some(config_b64) = secrets
            .get("config_b64")
            .and_then(SecretValue::as_string)
            .or_else(|| config.get("config_b64").map(String::as_str))
        {
            if secrets.get("config_b64").is_none() {
                warn!("secret value [config_b64] was not found, but present in configuration. Please prefer using secrets for sensitive values.");
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(config_b64.as_bytes())
                .context("invalid base64 encoding")?;
            serde_json::from_slice::<StorageConfig>(&bytes).context("corrupt config_b64")?
        } else if let Some(encoded) = secrets
            .get("config_json")
            .and_then(SecretValue::as_string)
            .or_else(|| config.get("config_json").map(String::as_str))
        {
            if secrets.get("config_json").is_none() {
                warn!("secret value [config_json] was not found, but was present in configuration. Please prefer using secrets for sensitive values.");
            }
            serde_json::from_str::<StorageConfig>(encoded).context("corrupt config_json")?
        } else {
            StorageConfig::default()
        };

        // If a top level BUCKET_REGION was specified config, use it
        if let Some(region) = config.get("BUCKET_REGION") {
            storage_config.bucket_region = Some(region.into());
        }

        if let Ok(arn) = env::var("AWS_ROLE_ARN") {
            let mut sts_config = storage_config.sts_config.unwrap_or_default();
            sts_config.role = arn;
            if let Ok(region) = env::var("AWS_ROLE_REGION") {
                sts_config.region = Some(region);
            }
            if let Ok(session) = env::var("AWS_ROLE_SESSION_NAME") {
                sts_config.session = Some(session);
            }
            if let Ok(external_id) = env::var("AWS_ROLE_EXTERNAL_ID") {
                sts_config.external_id = Some(external_id);
            }
            storage_config.sts_config = Some(sts_config);
        }

        if let Ok(endpoint) = env::var("AWS_ENDPOINT") {
            storage_config.endpoint = Some(endpoint);
        }

        // aliases are added from linkdefs in StorageClient::new()
        Ok(storage_config)
    }
}

#[derive(Clone)]
pub struct StorageClient {
    s3_client: aws_sdk_s3::Client,
    aliases: Arc<HashMap<String, String>>,
    /// Preferred region for bucket creation
    bucket_region: Option<BucketLocationConstraint>,
}

impl StorageClient {
    pub async fn new(
        StorageConfig {
            access_key_id,
            secret_access_key,
            session_token,
            region,
            max_attempts,
            sts_config,
            endpoint,
            mut aliases,
            bucket_region,
        }: StorageConfig,
        config_values: &HashMap<String, String>,
    ) -> Self {
        let region = match region {
            Some(region) => Some(Region::new(region)),
            _ => DefaultRegionChain::builder().build().region().await,
        };

        // use static credentials or defaults from environment
        let mut cred_provider = match (access_key_id, secret_access_key) {
            (Some(access_key_id), Some(secret_access_key)) => {
                SharedCredentialsProvider::new(aws_sdk_s3::config::Credentials::new(
                    access_key_id,
                    secret_access_key,
                    session_token,
                    None,
                    "static",
                ))
            }
            _ => SharedCredentialsProvider::new(
                DefaultCredentialsChain::builder()
                    .region(region.clone())
                    .build()
                    .await,
            ),
        };
        if let Some(StsAssumeRoleConfig {
            role,
            region,
            session,
            external_id,
        }) = sts_config
        {
            let mut role = AssumeRoleProvider::builder(role)
                .session_name(session.unwrap_or_else(|| DEFAULT_STS_SESSION.to_string()));
            if let Some(region) = region {
                role = role.region(Region::new(region));
            }
            if let Some(external_id) = external_id {
                role = role.external_id(external_id);
            }
            cred_provider = SharedCredentialsProvider::new(role.build().await);
        }

        let mut retry_config = RetryConfig::standard();
        if let Some(max_attempts) = max_attempts {
            retry_config = retry_config.with_max_attempts(max_attempts);
        }
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::v2024_03_28())
            .region(region)
            .credentials_provider(cred_provider)
            .retry_config(retry_config);
        if let Some(endpoint) = endpoint {
            loader = loader.endpoint_url(endpoint);
        };
        let s3_client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::from(&loader.load().await)
                .to_builder()
                // Since minio requires force path style,
                // turn it on since it's disabled by default
                // due to deprecation by AWS.
                // https://github.com/awslabs/aws-sdk-rust/issues/390
                .force_path_style(true)
                .http_client(
                    HyperClientBuilder::new().build(
                        hyper_rustls::HttpsConnectorBuilder::new()
                            .with_tls_config(
                                // use `tls::DEFAULT_CLIENT_CONFIG` directly once `rustls` versions
                                // are in sync
                                rustls::ClientConfig::builder()
                                    .with_root_certificates(rustls::RootCertStore {
                                        roots: tls::DEFAULT_ROOTS.roots.clone(),
                                    })
                                    .with_no_client_auth(),
                            )
                            .https_or_http()
                            .enable_all_versions()
                            .build(),
                    ),
                )
                .build(),
        );

        // Process aliases
        for (k, v) in config_values {
            if let Some(alias) = k.strip_prefix(ALIAS_PREFIX) {
                if alias.is_empty() || v.is_empty() {
                    error!("invalid bucket alias_ key and value must not be empty");
                } else {
                    aliases.insert(alias.to_string(), v.to_string());
                }
            }
        }

        StorageClient {
            s3_client,
            aliases: Arc::new(aliases),
            bucket_region: bucket_region.and_then(|v| BucketLocationConstraint::from_str(&v).ok()),
        }
    }

    /// perform alias lookup on bucket name
    /// This can be used either for giving shortcuts to actors in the linkdefs, for example:
    /// - component could use bucket names `alias_today`, `alias_images`, etc. and the linkdef aliases
    ///   will remap them to the real bucket name
    ///
    /// The `'alias_'` prefix is not required, so this also works as a general redirect capability
    pub fn unalias<'n, 's: 'n>(&'s self, bucket_or_alias: &'n str) -> &'n str {
        debug!(%bucket_or_alias, aliases = ?self.aliases);
        let name = bucket_or_alias
            .strip_prefix(ALIAS_PREFIX)
            .unwrap_or(bucket_or_alias);
        if let Some(name) = self.aliases.get(name) {
            name.as_ref()
        } else {
            name
        }
    }

    /// Check whether a container exists
    #[instrument(level = "debug", skip(self))]
    pub async fn container_exists(&self, bucket: &str) -> anyhow::Result<bool> {
        match self.s3_client.head_bucket().bucket(bucket).send().await {
            Ok(_) => Ok(true),
            Err(se) => match se.into_service_error() {
                HeadBucketError::NotFound(_) => Ok(false),
                err => {
                    error!(?err, code = err.code(), "Unable to head bucket");
                    bail!(anyhow!(err).context("failed to `head` bucket"))
                }
            },
        }
    }

    /// Create a bucket
    #[instrument(level = "debug", skip(self))]
    pub async fn create_container(&self, bucket: &str) -> anyhow::Result<()> {
        // Build bucket config, using location constraint if necessary
        let bucket_config = CreateBucketConfiguration::builder()
            .set_location_constraint(self.bucket_region.clone())
            .build();

        match self
            .s3_client
            .create_bucket()
            .create_bucket_configuration(bucket_config)
            .bucket(bucket)
            .send()
            .await
        {
            Ok(CreateBucketOutput { location, .. }) => {
                debug!(?location, "bucket created");
                Ok(())
            }
            Err(se) => match se.into_service_error() {
                CreateBucketError::BucketAlreadyOwnedByYou(..) => Ok(()),
                err => {
                    error!(?err, code = err.code(), "failed to create bucket");
                    bail!(anyhow!(err).context("failed to create bucket"))
                }
            },
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_container_info(&self, bucket: &str) -> anyhow::Result<ContainerMetadata> {
        match self.s3_client.head_bucket().bucket(bucket).send().await {
            Ok(_) => Ok(ContainerMetadata {
                // unfortunately, HeadBucketOut doesn't include any information
                // so we can't fill in creation date
                created_at: 0,
            }),
            Err(se) => match se.into_service_error() {
                HeadBucketError::NotFound(_) => {
                    error!("bucket [{bucket}] not found");
                    bail!("bucket [{bucket}] not found")
                }
                err => {
                    error!(?err, code = err.code(), "unexpected error");
                    bail!(anyhow!(err).context("unexpected error"));
                }
            },
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn list_container_objects(
        &self,
        bucket: &str,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> anyhow::Result<impl Iterator<Item = String>> {
        // TODO: Stream names
        match self
            .s3_client
            .list_objects_v2()
            .bucket(bucket)
            .set_max_keys(limit.map(|limit| limit.try_into().unwrap_or(i32::MAX)))
            .send()
            .await
        {
            Ok(ListObjectsV2Output { contents, .. }) => Ok(contents
                .into_iter()
                .flatten()
                .filter_map(|Object { key, .. }| key)
                .skip(offset.unwrap_or_default().try_into().unwrap_or(usize::MAX))
                .take(limit.unwrap_or(u64::MAX).try_into().unwrap_or(usize::MAX))),
            Err(SdkError::ServiceError(err)) => {
                error!(?err, "service error");
                bail!(anyhow!("{err:?}").context("service error"))
            }
            Err(err) => {
                error!(%err, code = err.code(), "unexpected error");
                bail!(anyhow!("{err:?}").context("unexpected error"))
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn copy_object(
        &self,
        src_bucket: &str,
        src_key: &str,
        dest_bucket: &str,
        dest_key: &str,
    ) -> anyhow::Result<()> {
        self.s3_client
            .copy_object()
            .copy_source(format!("{src_bucket}/{src_key}"))
            .bucket(dest_bucket)
            .key(dest_key)
            .send()
            .await
            .context("failed to copy object")?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self, object))]
    pub async fn delete_object(&self, container: &str, object: String) -> anyhow::Result<()> {
        self.s3_client
            .delete_object()
            .bucket(container)
            .key(object)
            .send()
            .await
            .context("failed to delete object")?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self, objects))]
    pub async fn delete_objects(
        &self,
        container: &str,
        objects: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<()> {
        let objects: Vec<_> = objects
            .into_iter()
            .map(|key| ObjectIdentifier::builder().key(key).build())
            .collect::<Result<_, _>>()
            .context("failed to build object identifier list")?;
        if objects.is_empty() {
            debug!("no objects to delete, return");
            return Ok(());
        }
        let delete = Delete::builder()
            .set_objects(Some(objects))
            .build()
            .context("failed to build `delete_objects` command")?;
        let out = self
            .s3_client
            .delete_objects()
            .bucket(container)
            .delete(delete)
            .send()
            .await
            .context("failed to delete objects")?;
        let errs = out.errors();
        if !errs.is_empty() {
            bail!("failed with errors {errs:?}")
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn delete_container(&self, bucket: &str) -> anyhow::Result<()> {
        match self.s3_client.delete_bucket().bucket(bucket).send().await {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(err)) => {
                bail!("{err:?}")
            }
            Err(err) => {
                error!(%err, code = err.code(), "unexpected error");
                bail!(err)
            }
        }
    }

    /// Find out whether object exists
    #[instrument(level = "debug", skip(self))]
    pub async fn has_object(&self, bucket: &str, key: &str) -> anyhow::Result<bool> {
        match self
            .s3_client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(se) => match se.into_service_error() {
                HeadObjectError::NotFound(_) => Ok(false),
                err => {
                    error!(
                        %err,
                        code = err.code(),
                        "unexpected error for object_exists"
                    );
                    bail!(anyhow!(err).context("unexpected error for object_exists"))
                }
            },
        }
    }

    /// Retrieves metadata about the object
    #[instrument(level = "debug", skip(self))]
    pub async fn get_object_info(&self, bucket: &str, key: &str) -> anyhow::Result<ObjectMetadata> {
        match self
            .s3_client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
        {
            Ok(HeadObjectOutput { content_length, .. }) => {
                Ok(ObjectMetadata {
                    // NOTE: The `created_at` value is not reported by S3
                    created_at: 0,
                    size: content_length
                        .and_then(|v| v.try_into().ok())
                        .unwrap_or_default(),
                })
            }
            Err(se) => match se.into_service_error() {
                HeadObjectError::NotFound(_) => {
                    error!("object [{bucket}/{key}] not found");
                    bail!("object [{bucket}/{key}] not found")
                }
                err => {
                    error!(
                        ?err,
                        code = err.code(),
                        "get_object_metadata failed for object [{bucket}/{key}]"
                    );
                    bail!(anyhow!(err).context(format!(
                        "get_object_metadata failed for object [{bucket}/{key}]"
                    )))
                }
            },
        }
    }
}

/// Blobstore S3 provider
///
/// This struct will be the target of generated implementations (via wit-provider-bindgen)
/// for the blobstore provider WIT contract
#[derive(Default, Clone)]
pub struct BlobstoreS3Provider {
    /// Per-component storage for NATS connection clients
    actors: Arc<RwLock<HashMap<String, StorageClient>>>,
}

pub async fn run() -> anyhow::Result<()> {
    BlobstoreS3Provider::run().await
}

impl BlobstoreS3Provider {
    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            "blobstore-s3-provider",
            std::env::var_os("PROVIDER_BLOBSTORE_S3_FLAMEGRAPH_PATH")
        );

        let provider = Self::default();
        let shutdown = run_provider(provider.clone(), "blobstore-s3-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        serve_provider_exports(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
            bindings::serve,
        )
        .await
        .context("failed to serve provider exports")
    }

    /// Retrieve the per-component [`StorageClient`] for a given link context
    async fn client(&self, context: Option<Context>) -> Result<StorageClient> {
        if let Some(ref source_id) = context.and_then(|Context { component, .. }| component) {
            self.actors
                .read()
                .await
                .get(source_id)
                .with_context(|| format!("failed to lookup {source_id} configuration"))
                .cloned()
        } else {
            // TODO: Support a default here
            bail!("failed to lookup invocation source ID")
        }
    }
}

impl bindings::exports::wrpc::blobstore::blobstore::Handler<Option<Context>>
    for BlobstoreS3Provider
{
    #[instrument(level = "trace", skip(self))]
    async fn clear_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            let bucket = client.unalias(&name);
            let objects = client
                .list_container_objects(bucket, None, None)
                .await
                .context("failed to list container objects")?;
            client.delete_objects(bucket, objects).await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn container_exists(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client.container_exists(client.unalias(&name)).await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn create_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client.create_container(client.unalias(&name)).await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client.delete_container(client.unalias(&name)).await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_container_info(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<ContainerMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client.get_container_info(client.unalias(&name)).await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn list_container_objects(
        &self,
        cx: Option<Context>,
        name: String,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            let names = client
                .list_container_objects(client.unalias(&name), limit, offset)
                .await
                .map(Vec::from_iter)?;
            anyhow::Ok((
                Box::pin(stream::iter([names])) as Pin<Box<dyn Stream<Item = _> + Send>>,
                Box::pin(async move { Ok(()) }) as Pin<Box<dyn Future<Output = _> + Send>>,
            ))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn copy_object(
        &self,
        cx: Option<Context>,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            let src_bucket = client.unalias(&src.container);
            let dest_bucket = client.unalias(&dest.container);
            client
                .copy_object(src_bucket, &src.object, dest_bucket, &dest.object)
                .await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_object(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client
                .delete_object(client.unalias(&id.container), id.object)
                .await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_objects(
        &self,
        cx: Option<Context>,
        container: String,
        objects: Vec<String>,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client
                .delete_objects(client.unalias(&container), objects)
                .await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_container_data(
        &self,
        cx: Option<Context>,
        id: ObjectId,
        start: u64,
        end: u64,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Bytes> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let limit = end
                .checked_sub(start)
                .context("`end` must be greater than `start`")?;
            let client = self.client(cx).await?;
            let bucket = client.unalias(&id.container);
            let GetObjectOutput { body, .. } = client
                .s3_client
                .get_object()
                .bucket(bucket)
                .key(id.object)
                .range(format!("bytes={start}-{end}"))
                .send()
                .await
                .context("failed to get object")?;
            let mut data = ReaderStream::new(body.into_async_read().take(limit));
            let (tx, rx) = mpsc::channel(16);
            anyhow::Ok((
                Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>,
                Box::pin(async move {
                    while let Some(buf) = data.next().await {
                        let buf = buf
                            .context("failed to read object")
                            .map_err(|err| format!("{err:#}"))?;
                        if tx.send(buf).await.is_err() {
                            return Err("stream receiver closed".to_string());
                        }
                    }
                    Ok(())
                }) as Pin<Box<dyn Future<Output = _> + Send>>,
            ))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_object_info(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<ObjectMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client
                .get_object_info(client.unalias(&id.container), &id.object)
                .await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn has_object(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            client
                .has_object(client.unalias(&id.container), &id.object)
                .await
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn move_object(
        &self,
        cx: Option<Context>,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            let src_bucket = client.unalias(&src.container);
            let dest_bucket = client.unalias(&dest.container);
            client
                .copy_object(src_bucket, &src.object, dest_bucket, &dest.object)
                .await
                .context("failed to copy object")?;
            client
                .delete_object(src_bucket, src.object)
                .await
                .context("failed to delete source object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self, data))]
    async fn write_container_data(
        &self,
        cx: Option<Context>,
        id: ObjectId,
        data: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> anyhow::Result<Result<Pin<Box<dyn Future<Output = Result<(), String>> + Send>>, String>>
    {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self.client(cx).await?;
            let req = client
                .s3_client
                .put_object()
                .bucket(client.unalias(&id.container))
                .key(&id.object);
            anyhow::Ok(Box::pin(async {
                // TODO: Stream data to S3
                let data: BytesMut = data.collect().await;
                req.body(data.freeze().into())
                    .send()
                    .await
                    .context("failed to put object")
                    .map_err(|err| format!("{err:#}"))?;
                Ok(())
            }) as Pin<Box<dyn Future<Output = _> + Send>>)
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }
}

/// Handle provider control commands
/// `put_link` (new component link command), `del_link` (remove link command), and shutdown
impl Provider for BlobstoreS3Provider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn receive_link_config_as_target(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        // Build storage config
        let config = match StorageConfig::from_link_config(&link_config).await {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, %link_config.source_id, "failed to build storage config");
                return Err(anyhow!(e).context("failed to build source config"));
            }
        };

        let link = StorageClient::new(config, link_config.config).await;

        let mut update_map = self.actors.write().await;
        update_map.insert(link_config.source_id.to_string(), link);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        let mut aw = self.actors.write().await;
        aw.remove(component_id);
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut aw = self.actors.write().await;
        // empty the component link data and stop all servers
        aw.drain();
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn aliases() {
        let client = StorageClient::new(
            StorageConfig::default(),
            &HashMap::from([(format!("{ALIAS_PREFIX}foo"), "bar".into())]),
        )
        .await;

        // no alias
        assert_eq!(client.unalias("boo"), "boo");
        // alias without prefix
        assert_eq!(client.unalias("foo"), "bar");
        // alias with prefix
        assert_eq!(client.unalias(&format!("{ALIAS_PREFIX}foo")), "bar");
        // undefined alias
        assert_eq!(client.unalias(&format!("{ALIAS_PREFIX}baz")), "baz");
    }
}
