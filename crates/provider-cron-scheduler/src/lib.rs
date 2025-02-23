use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, instrument, warn};
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{get_connection, run_provider, LinkConfig, LinkDeleteInfo, Provider};
use wit_bindgen_wrpc::bytes;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasmcloud:cron/scheduler@0.1.0": generate,
        }
    });
}

/// Configuration key for cron jobs
const CRON_JOBS_CONFIG_KEY: &str = "jobs";

/// Represents a unique identifier for a link (target_id, link_name)
#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct LinkId {
    pub target_id: String,
    pub link_name: String,
}

/// Unique identifier for a specific cron job
#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct CronJobId {
    pub link_id: LinkId,
    pub job_name: String,
}

/// Structure to hold cron job configuration
#[derive(Clone, Debug)]
struct CronJobConfig {
    pub expression: String,
    pub payload: Option<String>,
}

/// Type for storing cron tasks associated with jobs
type CronTaskMap = HashMap<CronJobId, JoinHandle<()>>;

/// Cron provider implementation for wasmcloud:cron scheduler
#[derive(Clone)]
pub struct CronProvider {
    // Store cron expressions and payloads per job
    cron_jobs: Arc<RwLock<HashMap<CronJobId, CronJobConfig>>>,
    // Store background tasks that handle scheduled cron jobs for each job
    cron_tasks: Arc<RwLock<CronTaskMap>>,
}

pub async fn run() -> anyhow::Result<()> {
    run_provider(CronProvider::new(), CronProvider::name())
        .await?
        .await;
    Ok(())
}

impl CronProvider {
    pub fn name() -> &'static str {
        "cron-scheduler-provider"
    }

    #[must_use]
    pub fn new() -> Self {
        CronProvider {
            cron_jobs: Arc::default(),
            cron_tasks: Arc::default(),
        }
    }
}

#[instrument(level = "info", skip(wrpc))]
async fn invoke_timed_job(wrpc: &WrpcClient, payload: Option<Bytes>) {
    let mut cx: async_nats::HeaderMap = async_nats::HeaderMap::new();
    for (k, v) in
        wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector::default_with_span(
        )
        .iter()
    {
        cx.insert(k.as_str(), v.as_str())
    }

    let payload_data = payload.unwrap_or_else(|| Bytes::from("{}"));

    match bindings::wasmcloud::cron::scheduler::invoke(wrpc, Some(cx), &payload_data).await {
        std::result::Result::Ok(res) => {
            debug!("successfully invoked timed job : {:?}", res);
        }
        Err(err) => {
            error!(?err, "failed to invoke timed job");
        }
    }
}

impl Provider for CronProvider {
    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id,
            config,
            secrets: _,
            link_name,
            wit_metadata: (_, _, interfaces),
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let component_id: Arc<str> = target_id.into();
        let wrpc = get_connection()
            .get_wrpc_client(&component_id)
            .await
            .context("failed to construct wRPC client")?;

        if interfaces.contains(&"scheduler".to_string()) {
            let wrpc = Arc::new(wrpc);
            let link_id = LinkId {
                target_id: target_id.to_string(),
                link_name: link_name.to_string(),
            };

            // Extract cron jobs from config
            let jobs_config = match config.get(CRON_JOBS_CONFIG_KEY) {
                Some(jobs) => jobs.trim(),
                None => {
                    warn!("No cron jobs found in link configuration for {}", target_id);
                    return Ok(());
                }
            };

            // Parse the jobs - format is job_name=cron_expression:payload
            let job_configs = self.parse_job_configs(jobs_config, &link_id)?;

            // Cancel any existing tasks for this link
            self.cancel_existing_tasks(&link_id).await;

            // Store new job configurations and start tasks
            let mut cron_jobs = self.cron_jobs.write().await;
            let mut tasks = self.cron_tasks.write().await;

            for (job_id, job_config) in job_configs {
                info!(
                    "Registering cron job '{}' for component {} with expression '{}'",
                    job_id.job_name, target_id, job_config.expression
                );

                // Store the job configuration
                cron_jobs.insert(job_id.clone(), job_config.clone());

                // Start a new task for this job
                let task = self.spawn_cron_job_task(job_id.clone(), job_config, wrpc.clone());

                tasks.insert(job_id, task);
            }
        }

        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        let link_id = LinkId {
            target_id: component_id.to_string(),
            link_name: link_name.to_string(),
        };

        // Cancel all tasks for this link
        self.cancel_existing_tasks(&link_id).await;

        // Remove all job configurations for this link
        let mut cron_jobs = self.cron_jobs.write().await;
        cron_jobs.retain(|job_id, _| job_id.link_id != link_id);

        info!("Cancelled all cron tasks for {}", component_id);
        Ok(())
    }

    /// Handle shutdown request by stopping all cron jobs
    async fn shutdown(&self) -> anyhow::Result<()> {
        info!("Shutting down cron provider");

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

// Implementation of cron scheduling logic
impl CronProvider {
    /// Parse job configurations from string
    fn parse_job_configs(
        &self,
        jobs_config: &str,
        link_id: &LinkId,
    ) -> anyhow::Result<HashMap<CronJobId, CronJobConfig>> {
        let mut job_configs = HashMap::new();

        for job_line in jobs_config.lines() {
            let job_line = job_line.trim();
            if job_line.is_empty() {
                continue;
            }

            // Split into name and configuration
            let parts: Vec<&str> = job_line.splitn(2, '=').collect();
            if parts.len() != 2 {
                error!("Invalid job configuration format: '{}'. Expected JOB_NAME=CRON_EXPRESSION:PAYLOAD", job_line);
                continue;
            }

            let job_name = parts[0].trim().to_string();
            let job_config = parts[1].trim();

            // Split configuration into expression and payload
            let config_parts: Vec<&str> = job_config.splitn(2, ':').collect();
            if config_parts.is_empty() {
                error!(
                    "Invalid job configuration value: '{}'. Expected CRON_EXPRESSION:PAYLOAD",
                    job_config
                );
                continue;
            }

            let expression = config_parts[0].trim().to_string();
            let payload = if config_parts.len() > 1 {
                Some(config_parts[1].trim().to_string())
            } else {
                None
            };

            // Validate cron expression
            if let Err(e) = Schedule::from_str(&expression) {
                error!("Invalid cron expression '{}': {}", expression, e);
                continue;
            }

            // Create job ID and configuration
            let job_id = CronJobId {
                link_id: link_id.clone(),
                job_name,
            };

            let job_config = CronJobConfig {
                expression,
                payload,
            };

            job_configs.insert(job_id, job_config);
        }

        Ok(job_configs)
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

        // Cancel and remove each task
        for job_id in job_ids_to_remove {
            if let Some(task) = tasks.remove(&job_id) {
                task.abort();
                let _ = task.await;
                debug!("Cancelled task for job '{}'", job_id.job_name);
            }
        }
    }

    /// Spawn a new task for a cron job
    fn spawn_cron_job_task(
        &self,
        job_id: CronJobId,
        job_config: CronJobConfig,
        wrpc: Arc<WrpcClient>,
    ) -> JoinHandle<()> {
        let self_clone = self.clone();

        tokio::spawn(async move {
            self_clone.run_cron_job(job_id, job_config, wrpc).await;
        })
    }

    /// Runs the cron job according to its schedule
    async fn run_cron_job(
        &self,
        job_id: CronJobId,
        initial_config: CronJobConfig,
        wrpc: Arc<WrpcClient>,
    ) {
        let mut current_config = initial_config;

        // Main cron job loop
        loop {
            // Parse the cron expression
            let schedule = match Schedule::from_str(&current_config.expression) {
                Ok(schedule) => schedule,
                Err(e) => {
                    error!(
                        "Failed to parse cron expression '{}': {}",
                        current_config.expression, e
                    );
                    // Sleep for a while before checking again
                    sleep(Duration::from_secs(60)).await;
                    continue;
                }
            };

            // Calculate time until next invocation
            let now = Utc::now();
            let next_execution = match schedule.after(&now).next() {
                Some(time) => time,
                None => {
                    warn!(
                        "Could not determine next execution time for cron expression '{}'",
                        current_config.expression
                    );
                    // Sleep for a minute before retrying
                    sleep(Duration::from_secs(60)).await;
                    continue;
                }
            };

            // Calculate duration until next execution
            let duration_until_next = (next_execution - now).to_std().unwrap_or_else(|_| {
                warn!("Next execution time is in the past, executing immediately");
                Duration::from_secs(0)
            });

            debug!(
                "Next execution for job '{}' scheduled at {}, waiting for {:?}",
                job_id.job_name, next_execution, duration_until_next
            );

            // Wait until next execution time
            sleep(duration_until_next).await;

            // Check if the job still exists and get updated config if available
            let job_exists = {
                let jobs = self.cron_jobs.read().await;
                match jobs.get(&job_id) {
                    Some(config) => {
                        current_config = config.clone();
                        true
                    }
                    None => false,
                }
            };

            if !job_exists {
                info!("Cron job '{}' was removed, stopping task", job_id.job_name);
                break;
            }

            // Prepare payload
            let payload_bytes = current_config
                .payload
                .as_ref()
                .map(|p| Bytes::from(p.clone()));

            // Invoke the job
            debug!("Executing cron job '{}'", job_id.job_name);
            invoke_timed_job(&wrpc, payload_bytes).await;
        }
    }
}
