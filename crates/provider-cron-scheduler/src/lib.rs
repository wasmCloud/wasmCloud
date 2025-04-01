use anyhow::{Context as AnyhowContext, Result};
use bytes::Bytes;
use chrono::Utc;
use cron::Schedule;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, instrument, warn};
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{get_connection, run_provider, LinkConfig, LinkDeleteInfo, Provider};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasmcloud:cron/scheduler@0.1.0": generate,
        }
    });
}

const CRON_JOBS_CONFIG_KEY: &str = "cronjobs";

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
}

type CronTaskMap = HashMap<CronJobId, JoinHandle<()>>;

#[derive(Clone)]
pub struct CronProvider {
    cron_jobs: Arc<RwLock<HashMap<CronJobId, CronJobConfig>>>,
    cron_tasks: Arc<RwLock<CronTaskMap>>,
}

pub async fn run() -> Result<()> {
    CronProvider::run().await
}

impl CronProvider {
    pub async fn run() -> Result<()> {
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
    let mut cx = async_nats::header::HeaderMap::new();
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
    ) -> Result<()> {
        let component_id: Arc<str> = target_id.into();
        let wrpc = get_connection()
            .get_wrpc_client(&component_id)
            .await
            .context(format!(
                "Failed to construct wRPC client for component {}",
                component_id
            ))?;

        if interfaces.contains(&"scheduler".to_string()) {
            let wrpc = Arc::new(wrpc);
            let link_id = LinkId {
                target_id: target_id.to_string(),
                link_name: link_name.to_string(),
            };

            // Extract the cron jobs from config
            let jobs_config = match config.get(CRON_JOBS_CONFIG_KEY) {
                Some(jobs) => jobs.trim(),
                None => {
                    warn!("No cron jobs found in link configuration for {}", target_id);
                    return Ok(());
                }
            };

            // Parse the jobs - format is job_name=cron_expression:payload
            let job_configs = self
                .parse_job_configs(jobs_config, &link_id)
                .context(format!(
                    "Failed to parse job configurations for {}",
                    target_id
                ))?;

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
                let task = self
                    .spawn_cron_job_task(job_id.clone(), job_config, wrpc.clone())
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

        // Remove all job configurations for this link
        let mut cron_jobs = self.cron_jobs.write().await;
        cron_jobs.retain(|job_id, _| job_id.link_id != link_id);

        info!("Cancelled all cron tasks for {}", component_id);
        Ok(())
    }

    /// Handle shutdown request by stopping all cron jobs
    async fn shutdown(&self) -> Result<()> {
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
    ) -> Result<JoinHandle<()>> {
        // Validate cron expression
        let schedule = Schedule::from_str(&job_config.expression).context(format!(
            "Failed to parse cron expression: '{}'",
            job_config.expression
        ))?;

        let job_name = job_id.job_name.clone();
        let payload = job_config.payload.as_ref().map(|s| Bytes::from(s.clone()));

        Ok(tokio::spawn(async move {
            info!(
                "Starting cron task for job '{}' with expression '{}'",
                job_name, job_config.expression
            );

            loop {
                // Calculate the next execution time based on the current time
                let now = Utc::now();
                let next_exec = match schedule.upcoming(Utc).next() {
                    Some(next) => next,
                    None => {
                        error!(
                            "Could not determine next execution time for job '{}'",
                            job_name
                        );
                        sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };

                // Calculate duration until next execution
                let duration_until_next = match (next_exec - now).to_std() {
                    Ok(duration) => duration,
                    Err(e) => {
                        error!(
                            "Failed to calculate duration until next execution for job '{}': {}",
                            job_name, e
                        );
                        sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };

                debug!(
                    "Next execution of job '{}' scheduled at {} (in {:?})",
                    job_name, next_exec, duration_until_next
                );

                // Sleep until the next execution time
                sleep(duration_until_next).await;

                // Job Execution
                info!("Executing cron job '{}' at {}", job_name, Utc::now());
                if let Err(e) = invoke_timed_job(&wrpc, &job_name, payload.clone()).await {
                    error!(
                        "Error executing cron job '{}': {}. Will retry at next scheduled time.",
                        job_name, e
                    );
                }
            }
        }))
    }
}
