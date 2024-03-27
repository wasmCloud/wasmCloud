//! blobstore-s3 capability provider
//!
//! This capability provider exposes [S3](https://aws.amazon.com/s3/)-compatible object storage
//! (AKA "blob store") as a [wasmcloud capability](https://wasmcloud.com/docs/concepts/capabilities) which
//! can be used by actors on your lattice.
//!

use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use async_trait::async_trait;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::default_provider::region::DefaultRegionChain;
use aws_config::retry::RetryConfig;
use aws_config::sts::AssumeRoleProvider;
use aws_sdk_s3::config::{Region, SharedCredentialsProvider};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::create_bucket::{CreateBucketError, CreateBucketOutput};
use aws_sdk_s3::operation::get_object::GetObjectOutput;
use aws_sdk_s3::operation::head_bucket::HeadBucketError;
use aws_sdk_s3::operation::head_object::{HeadObjectError, HeadObjectOutput};
use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Output;
use aws_sdk_s3::types::{Delete, Object, ObjectIdentifier};
use aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder;
use base64::Engine as _;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use serde::Deserialize;
use tokio::fs;
use tokio::io::AsyncReadExt as _;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, instrument};
use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::interfaces::blobstore::Blobstore;
use wasmcloud_provider_sdk::{Context, LinkConfig, ProviderHandler, ProviderOperationResult};
use wrpc_transport::{AcceptedInvocation, Transmitter};

const ALIAS_PREFIX: &str = "alias_";
const DEFAULT_STS_SESSION: &str = "blobstore_s3_provider";

/// Configuration for connecting to S3.
///
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
    pub async fn from_values(values: &HashMap<String, String>) -> Result<StorageConfig> {
        let mut config = if let Some(config_b64) = values.get("config_b64") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(config_b64.as_bytes())
                .context("invalid base64 encoding")?;
            serde_json::from_slice::<StorageConfig>(&bytes).context("corrupt config_b64")?
        } else if let Some(config) = values.get("config_json") {
            serde_json::from_str::<StorageConfig>(config).context("corrupt config_json")?
        } else {
            StorageConfig::default()
        };
        // load environment variables from file
        if let Some(env_file) = values.get("env") {
            let data = fs::read_to_string(env_file)
                .await
                .with_context(|| format!("reading env file '{env_file}'"))?;
            simple_env_load::parse_and_set(&data, |k, v| env::set_var(k, v));
        }

        if let Ok(arn) = env::var("AWS_ROLE_ARN") {
            let mut sts_config = config.sts_config.unwrap_or_default();
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
            config.sts_config = Some(sts_config);
        }

        if let Ok(endpoint) = env::var("AWS_ENDPOINT") {
            config.endpoint = Some(endpoint)
        }

        // aliases are added from linkdefs in StorageClient::new()
        Ok(config)
    }
}

#[derive(Clone)]
pub struct StorageClient {
    s3_client: aws_sdk_s3::Client,
    aliases: Arc<HashMap<String, String>>,
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
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::v2023_11_09())
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
        }
    }

    /// perform alias lookup on bucket name
    /// This can be used either for giving shortcuts to actors in the linkdefs, for example:
    /// - actor could use bucket names "alias_today", "alias_images", etc. and the linkdef aliases
    ///   will remap them to the real bucket name
    /// The 'alias_' prefix is not required, so this also works as a general redirect capability
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
                    error!(?err, "Unable to head bucket");
                    bail!(anyhow!(err).context("failed to `head` bucket"))
                }
            },
        }
    }

    /// Create a bucket
    #[instrument(level = "debug", skip(self))]
    pub async fn create_container(&self, bucket: &str) -> anyhow::Result<()> {
        match self.s3_client.create_bucket().bucket(bucket).send().await {
            Ok(CreateBucketOutput { location, .. }) => {
                debug!(?location, "bucket created");
                Ok(())
            }
            Err(se) => match se.into_service_error() {
                CreateBucketError::BucketAlreadyOwnedByYou(..) => Ok(()),
                err => {
                    error!(?err, "failed to create bucket");
                    bail!(anyhow!(err).context("failed to create bucket"))
                }
            },
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_container_info(
        &self,
        bucket: &str,
    ) -> anyhow::Result<wrpc_interface_blobstore::ContainerMetadata> {
        match self.s3_client.head_bucket().bucket(bucket).send().await {
            Ok(_) => Ok(wrpc_interface_blobstore::ContainerMetadata {
                // unfortunately, HeadBucketOut doesn't include any information
                // so we can't fill in creation date
                created_at: 0,
            }),
            Err(se) => match se.into_service_error() {
                HeadBucketError::NotFound(_) => {
                    error!("bucket [{bucket}] not found");
                    bail!("bucket [{bucket}] not found")
                }
                e => {
                    error!("unexpected error: {e}");
                    bail!("unexpected error: {e}");
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
                .flat_map(|Object { key, .. }| key)
                .skip(offset.unwrap_or_default().try_into().unwrap_or(usize::MAX))
                .take(limit.unwrap_or(u64::MAX).try_into().unwrap_or(usize::MAX))),
            Err(SdkError::ServiceError(err)) => {
                error!(?err, "service error");
                bail!(anyhow!("{err:?}").context("service error"))
            }
            Err(err) => {
                error!(%err, "unexpected error");
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
                error!(%err, "unexpected error");
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
                        "unexpected error for object_exists"
                    );
                    bail!(anyhow!(err).context("unexpected error for object_exists"))
                }
            },
        }
    }

    /// Retrieves metadata about the object
    #[instrument(level = "debug", skip(self))]
    pub async fn get_object_info(
        &self,
        bucket: &str,
        key: &str,
    ) -> anyhow::Result<wrpc_interface_blobstore::ObjectMetadata> {
        match self
            .s3_client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
        {
            Ok(HeadObjectOutput { content_length, .. }) => {
                Ok(wrpc_interface_blobstore::ObjectMetadata {
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
                    error!("get_object_metadata failed for object [{bucket}/{key}]: {err}",);
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
    /// Per-actor storage for NATS connection clients
    actors: Arc<RwLock<HashMap<String, StorageClient>>>,
}

impl BlobstoreS3Provider {
    /// Retrieve the per-actor [`StorageClient`] for a given link context
    async fn client(&self, context: Option<Context>) -> Result<StorageClient> {
        if let Some(ref source_id) = context.and_then(|Context { actor, .. }| actor) {
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

impl Blobstore for BlobstoreS3Provider {
    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_clear_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    let bucket = client.unalias(&container);
                    let objects = client
                        .list_container_objects(bucket, None, None)
                        .await
                        .context("failed to list container objects")?;
                    client.delete_objects(bucket, objects).await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_container_exists<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client.container_exists(client.unalias(&container)).await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_create_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client.create_container(client.unalias(&container)).await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_delete_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client.delete_container(client.unalias(&container)).await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_get_container_info<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client.get_container_info(client.unalias(&container)).await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[allow(clippy::type_complexity)]
    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_list_container_objects<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (container, limit, offset),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, Option<u64>, Option<u64>), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client
                        .list_container_objects(client.unalias(&container), limit, offset)
                        .await
                        .map(Vec::from_iter)
                        .map(Some)
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_copy_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (src, dest),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_blobstore::ObjectId,
                wrpc_interface_blobstore::ObjectId,
            ),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    let src_bucket = client.unalias(&src.container);
                    let dest_bucket = client.unalias(&dest.container);
                    client
                        .copy_object(src_bucket, &src.object, dest_bucket, &dest.object)
                        .await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_delete_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client
                        .delete_object(client.unalias(&id.container), id.object)
                        .await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_delete_objects<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (container, objects),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, Vec<String>), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client
                        .delete_objects(client.unalias(&container), objects)
                        .await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_get_container_data<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (id, start, end),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (wrpc_interface_blobstore::ObjectId, u64, u64),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let limit = end
                        .checked_sub(start)
                        .context("`end` must be greater than `start`")?;
                    let client = self.client(context).await?;
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
                    let data =
                        ReaderStream::new(body.into_async_read().take(limit)).map(move |buf| {
                            let buf = buf.context("failed to read chunk")?;
                            // TODO: Remove the need for this wrapping
                            Ok(buf
                                .into_iter()
                                .map(wrpc_transport::Value::U8)
                                .map(Some)
                                .collect())
                        });
                    anyhow::Ok(wrpc_transport::Value::Stream(Box::pin(data)))
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_get_object_info<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client
                        .get_object_info(client.unalias(&id.container), &id.object)
                        .await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_has_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client
                        .has_object(client.unalias(&id.container), &id.object)
                        .await
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_move_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (src, dest),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_blobstore::ObjectId,
                wrpc_interface_blobstore::ObjectId,
            ),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(
        level = "debug",
        skip(self, result_subject, error_subject, transmitter, data)
    )]
    async fn serve_write_container_data<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (id, data),
            result_subject,
            error_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_blobstore::ObjectId,
                impl Stream<Item = anyhow::Result<Bytes>> + Send,
            ),
            Tx,
        >,
    ) {
        // TODO: Stream value to S3
        let data: BytesMut = match data.try_collect().await {
            Ok(data) => data,
            Err(err) => {
                error!(?err, "failed to receive value");
                if let Err(err) = transmitter
                    .transmit_static(error_subject, err.to_string())
                    .await
                {
                    error!(?err, "failed to transmit error")
                }
                return;
            }
        };
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self.client(context).await?;
                    client
                        .s3_client
                        .put_object()
                        .bucket(client.unalias(&id.container))
                        .key(&id.object)
                        .body(data.freeze().into())
                        .send()
                        .await
                        .context("failed to put object")?;
                    anyhow::Ok(())
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl ProviderHandler for BlobstoreS3Provider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let source_id = link_config.get_source_id();
        let config_values = link_config.get_config();

        // Build storage config
        let config = match StorageConfig::from_values(config_values).await {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, %source_id, "failed to build storage config");
                return Err(anyhow!(e).context("failed to build source config").into());
            }
        };

        let link = StorageClient::new(config, config_values).await;

        let mut update_map = self.actors.write().await;
        update_map.insert(source_id.to_string(), link);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;
        aw.remove(source_id);
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
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
        assert_eq!(client.unalias(&format!("{}foo", ALIAS_PREFIX)), "bar");
        // undefined alias
        assert_eq!(client.unalias(&format!("{}baz", ALIAS_PREFIX)), "baz");
    }
}
