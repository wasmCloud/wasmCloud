use anyhow::{anyhow, bail, Context as AnyhowContext, Result};
use async_nats_40::jetstream::consumer::push::Config;
use async_nats_40::jetstream::consumer::Consumer;
use async_nats_40::jetstream::stream::StorageType;
use async_nats_40::Client;
use bytes::Bytes;
use chrono::Utc;
use cron::Schedule;
use futures::StreamExt;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::fs;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;
use wascap::prelude::KeyPair;
use wasmcloud_core::messaging::ConnectionConfig;
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, run_provider, LinkConfig, LinkDeleteInfo, Provider,
};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasmcloud:cron/scheduler@0.1.0": generate,
        }
    });
}
const CRON_JOBS_CONFIG_KEY: &str = "cronjobs";
// Maximum duration a lock can be held by any instance during job execution
// This value (1000ms) defines the automatic expiration time for locks in the NATS KeyValue store
// IMPORTANT: Ensure that any job's maximum execution time is less than this TTL value to prevent
// concurrent executions of the same job by multiple instances if the job takes longer than expected
const LOCK_MAX_AGE_MILLIS: u64 = 1000;
// The Number of seconds that the subject delete marker message persists in the stream
const SUBJECT_DELETE_MARKER_TTL_SECS: u64 = 1;
// The amount of time that the server wait fromm the time a message is delivered, till all the task execution happens
// If the ack is not returned before this time frame, the message would be redelivered and cron job will be re-attempted.
// Should Ideally be equal to max-execution time or slightly greater.
const CONSUMER_ACK_MAX_WAIT_TIME_SECS: u64 = 10;

#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct LinkId {
    pub target_id: String,
    pub link_name: String,
}

#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct CronJobId {
    pub link_id: LinkId,
    pub job_name: String,
}

#[derive(Clone, Debug)]
struct CronJobConfig {
    pub expression: String,
    pub payload: Option<String>,
    pub stream_name: Option<String>, // Important for Stream operations and consumer fetching.
    pub job_type: Option<CronJobType>,
}

type CronTaskMap = HashMap<CronJobId, JoinHandle<()>>;

#[derive(Clone)]
pub struct CronProvider {
    cron_jobs: Arc<RwLock<HashMap<CronJobId, CronJobConfig>>>,
    cron_tasks: Arc<RwLock<CronTaskMap>>,
    nats_client: Arc<OnceLock<async_nats_40::Client>>,
    instance_id: String, // Unique identifier for this provider instance
}

/// Categorizes a cron job based on its expression pattern
#[derive(Debug, Clone, PartialEq)]
enum CronJobType {
    /// Regular intervals with a fixed period in seconds
    /// The u64 value represents seconds between executions
    FixedInterval(u64),

    /// Complex scheduling patterns that require calculating the next execution time
    /// Examples: "Run at 9am on weekdays", "Run on the 1st and 15th of each month"
    DynamicInterval,
}

pub async fn run() -> Result<()> {
    CronProvider::run().await
}

impl CronProvider {
    pub async fn run() -> Result<()> {
        initialize_observability!(
            "cron-scheduler-provider",
            std::env::var_os("PROVIDER_CRON_SCHEDULER_FLAMEGRAPH_PATH")
        );
        run_provider(CronProvider::new(), CronProvider::name())
            .await
            .context("Failed to run provider")?
            .await;
        Ok(())
    }

    pub fn name() -> &'static str {
        "cron-scheduler-provider"
    }

    #[must_use]
    pub fn new() -> Self {
        CronProvider {
            cron_jobs: Arc::default(),
            cron_tasks: Arc::default(),
            nats_client: Arc::new(OnceLock::new()),
            instance_id: Uuid::new_v4().to_string(),
        }
    }

    async fn connect(&self, cfg: ConnectionConfig) -> anyhow::Result<Client> {
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(&seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats_40::ConnectOptions::with_jwt(jwt.into_string(), move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats_40::AuthError::new) }
                })
            }
            (None, None) => async_nats_40::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = cfg.tls_ca.as_deref() {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = cfg.tls_ca_file.as_deref() {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Use the first visible cluster_uri
        let url = cfg.cluster_uris.first().unwrap();

        // Override inbox prefix if specified
        if let Some(prefix) = cfg.custom_inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix);
        }

        let client = opts
            .name("NATS Cron Facilitator") // allow this to show up uniquely in a NATS connection list
            .connect(url.as_ref())
            .await?;

        Ok(client)
    }

    /// Get jetstream context
    async fn get_jetstream(&self) -> Result<async_nats_40::jetstream::context::Context> {
        match self.nats_client.get() {
            Some(client) => Ok(async_nats_40::jetstream::new(client.clone())),
            None => Err(anyhow::anyhow!("NATS client not initialized")),
        }
    }

    /// Get NATS KV store for locks
    async fn get_lock_kv(&self) -> Result<async_nats_40::jetstream::kv::Store> {
        let js = self.get_jetstream().await?;

        // Try to create the KV bucket for locks if it doesn't exist
        match js
            .create_key_value(async_nats_40::jetstream::kv::Config {
                bucket: "cron_locks".to_string(),
                history: 1,
                max_age: Duration::from_millis(LOCK_MAX_AGE_MILLIS),
                storage: StorageType::Memory,
                ..Default::default()
            })
            .await
        {
            Ok(kv) => Ok(kv),
            Err(_) => {
                // Bucket might already exist, try to open it
                js.get_key_value("cron_locks")
                    .await
                    .context("Failed to open cron_locks KV bucket")
            }
        }
    }
}

impl Default for CronProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[instrument(level = "info", skip(wrpc))]
async fn invoke_timed_job(wrpc: &WrpcClient, job_name: &str, payload: Option<Bytes>) -> Result<()> {
    let mut cx = async_nats_39::header::HeaderMap::new();
    for (k, v) in
        wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector::default_with_span(
        )
        .iter()
    {
        cx.insert(k.as_str(), v.as_str())
    }
    let payload_data = payload.unwrap_or_else(|| Bytes::from("{}"));

    let _ = bindings::wasmcloud::cron::scheduler::invoke(wrpc, Some(cx), &payload_data)
        .await
        .map_err(|err| {
            error!(?err, job_name = job_name, "Failed to invoke timed job");
            anyhow::anyhow!("Failed to invoke timed job '{}': {:?}", job_name, err)
        })?;

    debug!(job_name = job_name, "Successfully invoked timed job");
    Ok(())
}

impl Provider for CronProvider {
    async fn receive_link_config_as_source(&self, linkconfig: LinkConfig<'_>) -> Result<()> {
        info!("Received Link Config as source, proceeding to connect to NATS");
        // Initialize NATS connection if not already done
        let config = ConnectionConfig::from_map(linkconfig.config)?;
        let client = match self.connect(config).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };

        let _ = self.nats_client.set(client);

        let component_id: Arc<str> = linkconfig.target_id.into();
        let wrpc = get_connection()
            .get_wrpc_client(&component_id)
            .await
            .context(format!(
                "Failed to construct wRPC client for component {}",
                component_id
            ))?;

        if linkconfig.wit_metadata.2.contains(&"scheduler".to_string()) {
            let wrpc = Arc::new(wrpc);
            let link_id = LinkId {
                target_id: linkconfig.target_id.to_string(),
                link_name: linkconfig.link_name.to_string(),
            };

            // Extract the cron jobs from config
            let jobs_config = match linkconfig.config.get(CRON_JOBS_CONFIG_KEY) {
                Some(jobs) => jobs.trim(),
                None => {
                    warn!(
                        "No cron jobs found in link configuration for {}",
                        linkconfig.target_id
                    );
                    return Ok(());
                }
            };

            // Parse the jobs - format is job_name=cron_expression:payload
            let job_configs = self
                .parse_job_configs(jobs_config, &link_id)
                .context(format!(
                    "Failed to parse job configurations for {}",
                    linkconfig.target_id
                ))?;

            // Cancel any existing tasks for this link
            self.cancel_existing_tasks(&link_id).await;

            // Store new job configurations and start tasks
            let mut cron_jobs = self.cron_jobs.write().await;
            let mut tasks = self.cron_tasks.write().await;

            for (job_id, mut job_config) in job_configs {
                info!(
                    "Registering cron job '{}' for component {} with expression '{}'",
                    job_id.job_name, linkconfig.target_id, job_config.expression
                );

                // Create a stream for this job - note we're passing a mutable reference to job_config
                // so that the job_type can be set during stream creation
                let stream_name = self.create_job_stream(&job_id, &mut job_config).await?;
                job_config.stream_name = Some(stream_name);

                // Store the job configuration
                cron_jobs.insert(job_id.clone(), job_config.clone());

                // Start a new task to monitor the job stream
                let task = self
                    .spawn_distributed_job_task(job_id.clone(), job_config, wrpc.clone())
                    .context(format!(
                        "Failed to spawn task for job '{}'",
                        job_id.job_name
                    ))?;

                tasks.insert(job_id, task);
            }
        }

        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        let link_id = LinkId {
            target_id: component_id.to_string(),
            link_name: link_name.to_string(),
        };

        // Cancel all tasks for this link
        self.cancel_existing_tasks(&link_id).await;

        // Delete all job streams for this link
        self.delete_job_streams(&link_id).await?;

        // Remove all job configurations for this link
        let mut cron_jobs = self.cron_jobs.write().await;
        cron_jobs.retain(|job_id, _| job_id.link_id != link_id);

        info!("Cancelled all cron tasks for {}", component_id);
        Ok(())
    }

    /// Handle shutdown request by stopping all cron jobs
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down distributed cron provider");

        // Cancel all running tasks
        let mut tasks = self.cron_tasks.write().await;
        for (job_id, task) in tasks.drain() {
            task.abort();
            let _ = task.await;
            debug!("Cancelled task for job '{}'", job_id.job_name);
        }

        // Clear job registry
        let mut cron_jobs = self.cron_jobs.write().await;
        cron_jobs.clear();

        Ok(())
    }
}

// Implementation of distributed cron scheduling logic
impl CronProvider {
    /// Parse job configurations from string
    fn parse_job_configs(
        &self,
        jobs_config: &str,
        link_id: &LinkId,
    ) -> Result<HashMap<CronJobId, CronJobConfig>> {
        let mut job_configs = HashMap::new();

        for job_line in jobs_config.lines() {
            let job_line = job_line.trim();
            if job_line.is_empty() {
                continue;
            }

            // Split into name and configuration
            let parts: Vec<&str> = job_line.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(anyhow::anyhow!(
                "Invalid job configuration format: '{}'. Expected JOB_NAME=CRON_EXPRESSION:PAYLOAD", 
                job_line
            ));
            }

            let job_name = parts[0].trim().to_string();
            let job_config = parts[1].trim();

            // Split configuration into expression and payload
            let config_parts: Vec<&str> = job_config.splitn(2, ':').collect();
            if config_parts.is_empty() {
                return Err(anyhow::anyhow!(
                    "Invalid job configuration value: '{}'. Expected CRON_EXPRESSION:PAYLOAD",
                    job_config
                ));
            }

            let expression = config_parts[0].trim().to_string();
            let payload = if config_parts.len() > 1 {
                Some(config_parts[1].trim().to_string())
            } else {
                None
            };

            // Validate cron expression
            Schedule::from_str(&expression)
                .context(format!("Invalid cron expression: '{}'", expression))?;

            let job_id = CronJobId {
                link_id: link_id.clone(),
                job_name,
            };

            let job_config = CronJobConfig {
                expression,
                payload,
                stream_name: None, // Will be set when stream is created
                job_type: None,    // Will be set during analysis in create_job_stream
            };

            job_configs.insert(job_id, job_config);
        }

        Ok(job_configs)
    }

    /// Calculate interval in seconds from cron expression
    async fn calculate_interval_from_cron(&self, expression: &str) -> Result<u64> {
        let schedule = Schedule::from_str(expression)?;

        // Get next two executions to calculate interval
        let mut upcoming = schedule.upcoming(Utc);
        let next1 = upcoming
            .next()
            .ok_or_else(|| anyhow::anyhow!("Could not determine next execution time"))?;
        let next2 = upcoming
            .next()
            .ok_or_else(|| anyhow::anyhow!("Could not determine second execution time"))?;

        // Calculate interval between executions in seconds
        let interval = (next2 - next1).num_seconds();
        if interval <= 0 {
            return Err(anyhow::anyhow!(
                "Invalid interval calculated from cron expression"
            ));
        }

        // Add small buffer
        let interval_with_buffer = (interval as f64 * 1.1) as u64;
        Ok(interval_with_buffer)
    }

    /// Create JetStream stream for a job
    async fn create_job_stream(
        &self,
        job_id: &CronJobId,
        job_config: &mut CronJobConfig,
    ) -> Result<String> {
        let js = self.get_jetstream().await?;

        // First, analyze the cron expression to determine its type
        let job_type = self.analyze_cron_expression(&job_config.expression).await?;

        // Store the job type for future reference
        job_config.job_type = Some(job_type.clone());

        // Calculate TTL based on job type
        let initial_ttl = match &job_type {
            CronJobType::FixedInterval(interval) => *interval,
            CronJobType::DynamicInterval => {
                self.time_until_next_execution(&job_config.expression)
                    .await?
            }
        };

        // Create a unique stream name for this job
        let stream_name = format!(
            "CRONJOB_{}_{}",
            job_id.link_id.target_id.replace("-", "_"),
            job_id.job_name
        );
        let subject_name = format!("cronjob.{}.{}", job_id.link_id.target_id, job_id.job_name);

        // Create the stream with calculated TTL
        // Configure with max_messages=1 to retain only the most recent message
        // When the TTL constraint is fulfilled (either because of stream.MaxAge or per-message TTL), the message will expire and
        // a subject delete marker will be propagated, allowing the next message
        // This configuration leverages NATS's idempotency for automatic deduplication
        let mut stream_config = async_nats_40::jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![subject_name.clone()],
            max_messages: 1,
            allow_message_ttl: true,
            subject_delete_marker_ttl: Some(Duration::from_secs(SUBJECT_DELETE_MARKER_TTL_SECS)),
            retention: async_nats_40::jetstream::stream::RetentionPolicy::Limits,
            storage: StorageType::Memory,
            discard: async_nats_40::jetstream::stream::DiscardPolicy::Old,
            num_replicas: 1,
            allow_rollup: true,
            allow_direct: true,
            ..Default::default()
        };

        // For fixed interval jobs, set max_age in the stream config
        if let CronJobType::FixedInterval(_) = job_type {
            stream_config.max_age = Duration::from_secs(initial_ttl);
        }

        // Create or get the stream
        let stream = match js.create_stream(stream_config.clone()).await {
            Ok(stream) => {
                debug!("Created stream {} for job {}", stream_name, job_id.job_name);
                stream
            }
            Err(e) => {
                // If the stream already exists, update its config
                if e.to_string().contains("already in use") {
                    // Then get the stream object
                    js.get_stream(&stream_name).await?
                } else {
                    return Err(anyhow::anyhow!("Failed to create stream: {}", e));
                }
            }
        };

        // Create a consumer for this job
        let consumer_name = format!("consumer_{}_{}", job_id.link_id.target_id, job_id.job_name);
        let delivery_subject = format!("delivery.{}.{}", job_id.link_id.target_id, job_id.job_name);

        // Create a durable push consumer
        let _consumer = stream
            .create_consumer(async_nats_40::jetstream::consumer::push::Config {
                durable_name: Some(consumer_name),
                deliver_subject: delivery_subject,
                deliver_group: Some(format!("cron_group_{}", job_id.job_name)),
                ack_policy: async_nats_40::jetstream::consumer::AckPolicy::Explicit,
                ack_wait: Duration::from_secs(CONSUMER_ACK_MAX_WAIT_TIME_SECS),
                max_deliver: 10,
                ..Default::default()
            })
            .await?;

        // Publish initial tick differently based on job type
        match job_type {
            CronJobType::FixedInterval(_) => {
                // For fixed interval jobs, publish without headers since max_age is set in the stream
                if let Err(e) = js.publish(subject_name, "tick".into()).await {
                    error!(
                        "Failed to publish tick for job '{}': {}",
                        job_id.job_name, e
                    );
                }
            }
            CronJobType::DynamicInterval => {
                // For dynamic interval jobs, use headers with TTL
                let ttl_duration = initial_ttl.to_string();
                let mut hmap = async_nats_40::HeaderMap::new();
                hmap.append(async_nats_40::header::NATS_MESSAGE_TTL, ttl_duration);

                // Initial tick to start the job cycle
                if let Err(e) = js
                    .publish_with_headers(subject_name, hmap, "tick".into())
                    .await
                {
                    error!(
                        "Failed to publish tick for job '{}': {}",
                        job_id.job_name, e
                    );
                }
            }
        }

        Ok(stream_name)
    }

    /// Delete streams associated with a link
    async fn delete_job_streams(&self, link_id: &LinkId) -> Result<()> {
        let jobs = self.cron_jobs.read().await;
        let js = self.get_jetstream().await?;

        for (job_id, job_config) in jobs.iter() {
            if job_id.link_id == *link_id {
                if let Some(stream_name) = &job_config.stream_name {
                    match js.delete_stream(stream_name).await {
                        Ok(_) => {
                            info!("Deleted stream {} for job {}", stream_name, job_id.job_name)
                        }
                        Err(e) => warn!("Failed to delete stream {}: {}", stream_name, e),
                    }
                }
            }
        }

        Ok(())
    }

    /// Cancel all existing tasks for a link
    async fn cancel_existing_tasks(&self, link_id: &LinkId) {
        let mut tasks = self.cron_tasks.write().await;

        // Find all tasks for this link
        let job_ids_to_remove: Vec<CronJobId> = tasks
            .keys()
            .filter(|job_id| job_id.link_id == *link_id)
            .cloned()
            .collect();

        for job_id in job_ids_to_remove {
            if let Some(task) = tasks.remove(&job_id) {
                task.abort();
                let _ = task.await;
                debug!("Cancelled task for job '{}'", job_id.job_name);
            }
        }
    }

    /// Try to acquire a distributed lock for job execution
    async fn try_acquire_lock(&self, job_id: &CronJobId) -> Result<bool> {
        let kv = self.get_lock_kv().await?;
        let lock_key = format!("lock/{}/{}", job_id.link_id.target_id, job_id.job_name);

        // Try to create the lock entry
        match kv
            .create(&lock_key, self.instance_id.as_bytes().to_vec().into())
            .await
        {
            Ok(_) => {
                debug!("Acquired lock for job '{}'", job_id.job_name);
                Ok(true)
            }
            Err(_) => {
                // Another instance has the lock
                debug!("Failed to acquire lock for job '{}'", job_id.job_name);
                Ok(false)
            }
        }
    }

    /// Spawn a new task for distributed job monitoring
    fn spawn_distributed_job_task(
        &self,
        job_id: CronJobId,
        job_config: CronJobConfig,
        wrpc: Arc<WrpcClient>,
    ) -> Result<JoinHandle<()>> {
        let job_name = job_id.job_name.clone();
        let payload = job_config.payload.as_ref().map(|s| Bytes::from(s.clone()));
        let stream_name = job_config
            .stream_name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Stream name not set for job {}", job_name))?;

        let nats_client = self.nats_client.clone();
        let provider_clone = self.clone();

        // Clone job_id for the task
        let job_id_clone = job_id.clone();
        // Clone job_config for the task
        let job_config_clone = job_config.clone();

        Ok(tokio::spawn(async move {
            info!(
                "Starting distributed cron task for job '{}' with stream '{}'",
                job_name, stream_name
            );

            let nats = loop {
                if let Some(client) = nats_client.get() {
                    break client;
                }
                sleep(Duration::from_secs(1)).await;
            };

            let js = async_nats_40::jetstream::new(nats.clone());

            // Get the stream
            let stream = match js.get_stream(&stream_name).await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Failed to get stream for job '{}': {}", job_name, e);
                    return;
                }
            };

            // Get the push consumer
            let consumer_name =
                format!("consumer_{}_{}", job_id.link_id.target_id, job_id.job_name);
            let consumer: Consumer<Config> = match stream.get_consumer(&consumer_name).await {
                Ok(consumer) => consumer,
                Err(e) => {
                    error!("Failed to get consumer for job '{}': {}", job_name, e);
                    return;
                }
            };

            // Get messages from the consumer
            let mut messages = match consumer.messages().await {
                Ok(msgs) => msgs,
                Err(e) => {
                    error!("Failed to get messages for job '{}': {}", job_name, e);
                    return;
                }
            };

            // Process messages (including delete markers)
            while let Some(msg) = messages.next().await {
                match msg {
                    Ok(msg) => {
                        // Check for delete marker
                        if let Some(headers) = msg.headers.as_ref() {
                            for (key, values) in headers.iter() {
                                if *key == async_nats_40::header::NATS_MARKER_REASON
                                    && !values.is_empty()
                                {
                                    if let Some(reason) = values.first() {
                                        if *reason == "MaxAge".into() {
                                            // Try to acquire lock
                                            match provider_clone
                                                .try_acquire_lock(&job_id_clone)
                                                .await
                                            {
                                                Ok(true) => {
                                                    debug!(
                                                        "Instance {} acquired lock for job '{}', executing",
                                                        provider_clone.instance_id, job_name
                                                    );

                                                    match invoke_timed_job(
                                                        &wrpc,
                                                        &job_name,
                                                        payload.clone(),
                                                    )
                                                    .await
                                                    {
                                                        Ok(_) => {
                                                            info!(
                                                                "Successfully executed job '{}'",
                                                                job_name
                                                            );
                                                        }
                                                        Err(e) => {
                                                            // Increase visibility of errors with warn level
                                                            warn!(
                                                                "Error executing cron job '{}': {}. Will retry at next interval.",
                                                                job_name, e
                                                            );
                                                        }
                                                    }

                                                    // Update TTL and republish
                                                    if let Some(CronJobType::DynamicInterval) =
                                                        job_config_clone.job_type
                                                    {
                                                        // Update TTL for dynamic interval jobs
                                                        if let Err(e) = provider_clone
                                                            .update_ttl_republish(
                                                                &job_id_clone,
                                                                &job_config_clone,
                                                                nats,
                                                            )
                                                            .await
                                                        {
                                                            error!(
                                                                "Failed to update TTL for job '{}': {}",
                                                                job_name, e
                                                            );
                                                        }
                                                    } else {
                                                        // For fixed interval jobs, just republish
                                                        let subject = format!(
                                                            "cronjob.{}.{}",
                                                            job_id_clone.link_id.target_id,
                                                            job_name
                                                        );
                                                        if let Err(e) = nats
                                                            .publish(subject, "tick".into())
                                                            .await
                                                        {
                                                            error!(
                                                                "Failed to publish tick for job '{}': {}",
                                                                job_name, e
                                                            );
                                                        }
                                                    }
                                                }
                                                Ok(false) => {
                                                    info!("Failed to acquire lock for job '{}', another instance will execute it", job_name);
                                                }
                                                Err(e) => {
                                                    error!(
                                                        "Error acquiring lock for job '{}': {}",
                                                        job_name, e
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Acknowledge the message to avoid redelivery
                        if let Err(e) = msg.ack().await {
                            warn!(
                                "Failed to acknowledge message for job '{}': {}. The job could be re-invoked.",
                                job_name, e
                            );
                        }
                    }
                    Err(e) => {
                        error!("Error receiving message for job '{}': {}", job_name, e);
                    }
                }
            }
        }))
    }

    /// Analyzes a cron expression and determines its type
    async fn analyze_cron_expression(&self, expression: &str) -> Result<CronJobType> {
        if has_fixed_interval(expression)? {
            // For fixed intervals, calculate the interval once
            let interval_secs = self.calculate_interval_from_cron(expression).await?;
            Ok(CronJobType::FixedInterval(interval_secs))
        } else {
            Ok(CronJobType::DynamicInterval)
        }
    }

    /// Calculate time until next execution from cron expression
    async fn time_until_next_execution(&self, expression: &str) -> Result<u64> {
        let schedule = Schedule::from_str(expression)?;
        let now = Utc::now();

        // Get next execution time
        let next = schedule
            .upcoming(Utc)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Could not determine next execution time"))?;

        // Calculate seconds until next execution
        let duration = next.signed_duration_since(now);
        if duration.num_seconds() <= 0 {
            return Err(anyhow::anyhow!("Next execution time is in the past"));
        }

        // Add small buffer (5% extra time)
        let seconds_with_buffer = (duration.num_seconds() as f64 * 1.05) as u64;
        Ok(seconds_with_buffer)
    }

    /// Update stream TTL and republish tick for dynamic interval jobs
    async fn update_ttl_republish(
        &self,
        job_id: &CronJobId,
        job_config: &CronJobConfig,
        nats: &async_nats_40::Client,
    ) -> Result<()> {
        let subject_name = format!("cronjob.{}.{}", job_id.link_id.target_id, job_id.job_name);
        // Calculate time until next execution
        let next_execution_secs = self
            .time_until_next_execution(&job_config.expression)
            .await?;
        debug!(
            "Republishing message with updated TTL of {} seconds for job '{}'",
            next_execution_secs, job_id.job_name
        );

        let ttl_duration = next_execution_secs.to_string();

        let mut hmap = async_nats_40::HeaderMap::new();
        hmap.append(async_nats_40::header::NATS_MESSAGE_TTL, ttl_duration);

        // Republish the tick that will expire at the right time
        if let Err(e) = nats
            .publish_with_headers(subject_name, hmap, "tick".into())
            .await
        {
            error!(
                "Failed to publish tick for job '{}': {}",
                job_id.job_name, e
            );
        }

        Ok(())
    }
}
fn has_fixed_interval(expression: &str) -> Result<bool> {
    let schedule = Schedule::from_str(expression)?;

    // Get the next few execution times
    let mut upcoming = schedule.upcoming(Utc);
    let mut intervals = Vec::new();

    // Collect 10 intervals to check for consistency
    let mut prev_time = upcoming
        .next()
        .ok_or_else(|| anyhow::anyhow!("Could not determine first execution time"))?;
    for _ in 0..10 {
        if let Some(next_time) = upcoming.next() {
            let interval = next_time.signed_duration_since(prev_time).num_seconds();
            intervals.push(interval);
            prev_time = next_time;
        } else {
            break;
        }
    }
    // Check if all intervals are the same (with a small tolerance for leap seconds, etc.)
    if intervals.len() >= 5 {
        let first_interval = intervals[0];
        let all_same = intervals.iter().all(|&i| (i - first_interval).abs() < 2); // 2 seconds tolerance
        Ok(all_same)
    } else {
        // Not enough intervals to determine, assume non-fixed
        Ok(false)
    }
}

fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats_40::ConnectOptions,
) -> anyhow::Result<async_nats_40::ConnectOptions> {
    let ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes()).context("failed to read CA")?;
    let mut roots = async_nats_40::rustls::RootCertStore::empty();
    if let Some(rustls_pemfile::Item::X509Certificate(ca)) = ca {
        roots.add_parsable_certificates([ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats_40::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    #[test]
    fn test_has_fixed_interval() {
        // Simple fixed intervals
        assert!(has_fixed_interval("0 */5 * * * *").unwrap()); // Every 5 minutes
        assert!(has_fixed_interval("0 0 */2 * * *").unwrap()); // Every 2 hours
        assert!(has_fixed_interval("0 0 0 * * *").unwrap()); // Daily at midnight

        // Complex non-fixed intervals
        assert!(!has_fixed_interval("0 0 9,12,15 * * *").unwrap()); // Specific hours
        assert!(!has_fixed_interval("0 0 0 1,15 * *").unwrap()); // 1st and 15th of month
    }

    #[test]
    fn test_parse_job_configs() {
        let provider = CronProvider::new();
        let link_id = LinkId {
            target_id: "test-component".to_string(),
            link_name: "test-link".to_string(),
        };

        // Valid job configuration with JSON payload
        let config = r#"job1=0 */5 * * * *:{"key":"value"}
job2=0 0 0 * * *"#;
        let result = provider.parse_job_configs(config, &link_id).unwrap();

        assert_eq!(result.len(), 2);

        let job1 = result
            .get(&CronJobId {
                link_id: link_id.clone(),
                job_name: "job1".to_string(),
            })
            .unwrap();

        assert_eq!(job1.expression, "0 */5 * * * *");
        assert_eq!(job1.payload, Some(r#"{"key":"value"}"#.to_string()));

        let job2 = result
            .get(&CronJobId {
                link_id: link_id.clone(),
                job_name: "job2".to_string(),
            })
            .unwrap();

        assert_eq!(job2.expression, "0 0 0 * * *");
        assert_eq!(job2.payload, None);

        // Valid job configuration with question mark
        let config = r#"demo=*/3 * * * * ?:{"x1":"x2"}"#;
        let result = provider.parse_job_configs(config, &link_id).unwrap();

        assert_eq!(result.len(), 1);
        let demo_job = result
            .get(&CronJobId {
                link_id: link_id.clone(),
                job_name: "demo".to_string(),
            })
            .unwrap();

        assert_eq!(demo_job.expression, "*/3 * * * * ?");
        assert_eq!(demo_job.payload, Some(r#"{"x1":"x2"}"#.to_string()));

        // Invalid job configuration - no expression
        let invalid_config = "job1=";
        let result = provider.parse_job_configs(invalid_config, &link_id);
        assert!(result.is_err());

        // Invalid job configuration - invalid expression
        let invalid_config = "job1=invalid cron:payload";
        let result = provider.parse_job_configs(invalid_config, &link_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_analyze_cron_expression() {
        let rt = Runtime::new().unwrap();
        let provider = CronProvider::new();

        // Test fixed interval expressions
        let fixed_expr = "0 */5 * * * *"; // Every 5 minutes
        let result = rt
            .block_on(provider.analyze_cron_expression(fixed_expr))
            .unwrap();
        match result {
            CronJobType::FixedInterval(interval) => {
                // Should be around 300 seconds (5 minutes) with some buffer
                assert!((300..=330).contains(&interval));
            }
            _ => panic!("Expected FixedInterval for expression: {}", fixed_expr),
        }

        // Test dynamic interval expressions
        let dynamic_expr = "0 0 9,17 * * 1-5"; // 9am and 5pm on weekdays
        let result = rt
            .block_on(provider.analyze_cron_expression(dynamic_expr))
            .unwrap();
        match result {
            CronJobType::DynamicInterval => {}
            _ => panic!("Expected DynamicInterval for expression: {}", dynamic_expr),
        }

        // Test expressions with question mark
        let question_mark_expr = "*/3 * * * * ?"; // Every 3 seconds
        let result = rt
            .block_on(provider.analyze_cron_expression(question_mark_expr))
            .unwrap();
        match result {
            CronJobType::FixedInterval(interval) => {
                // Should be around 3 seconds with some buffer
                assert!((3..=4).contains(&interval));
            }
            _ => panic!(
                "Expected FixedInterval for expression: {}",
                question_mark_expr
            ),
        }
    }

    #[test]
    fn test_time_until_next_execution() {
        let rt = Runtime::new().unwrap();
        let provider = CronProvider::new();

        // Test daily at midnight (with seconds)
        let result = rt
            .block_on(provider.time_until_next_execution("0 0 0 * * *"))
            .unwrap();
        // Result will depend on current time, but should be positive and reasonable
        assert!(result > 0 && result <= 86400); // Within 24 hours

        // Test every minute instead of every second (less prone to timing issues)
        let result = rt
            .block_on(provider.time_until_next_execution("0 * * * * *"))
            .unwrap();
        assert!(result > 0 && result <= 60); // Within a minute

        // Test with question mark for day of week - every hour
        let result = rt
            .block_on(provider.time_until_next_execution("0 0 * * * ?"))
            .unwrap();
        assert!(result > 0 && result <= 3600); // Within an hour
    }

    #[test]
    fn test_calculate_interval_from_cron() {
        let rt = Runtime::new().unwrap();
        let provider = CronProvider::new();

        // Test every 5 minutes
        let result = rt
            .block_on(provider.calculate_interval_from_cron("0 */5 * * * *"))
            .unwrap();
        // Should be around 300 seconds (5 minutes) with 10% buffer
        assert!((300..=330).contains(&result));

        // Test hourly
        let result = rt
            .block_on(provider.calculate_interval_from_cron("0 0 * * * *"))
            .unwrap();
        // Should be around 3600 seconds (1 hour) with 10% buffer
        assert!((3600..=3960).contains(&result));

        // Test daily
        let result = rt
            .block_on(provider.calculate_interval_from_cron("0 0 0 * * *"))
            .unwrap();
        // Should be around 86400 seconds (24 hours) with 10% buffer
        assert!((86400..=95040).contains(&result));

        // Test with question mark and seconds
        let result = rt
            .block_on(provider.calculate_interval_from_cron("*/3 * * * * ?"))
            .unwrap();
        // Should be around 3 seconds with 10% buffer
        assert!((3..=4).contains(&result));
    }
}
