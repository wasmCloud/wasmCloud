use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context as _, Result};
use chrono::{DateTime, Utc};
use console::style;
use rand::{distributions::Alphanumeric, Rng};
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt as _;
use tokio::process::Child;
use crate::lib::common::CommandGroupUsage;

use crate::lib::config::downloads_dir;
use crate::lib::generate::emoji;
use crate::lib::id::ServerId;
use crate::lib::start::{
    ensure_nats_server, ensure_wadm, ensure_wasmcloud, start_wadm, start_wasmcloud_host,
    NatsConfig, WadmConfig, NATS_SERVER_BINARY,
};

use crate::cmd::up::{remove_wadm_pidfile, start_nats, NatsOpts, WadmOpts, WasmcloudOpts};
use crate::config::{configure_host_env, DEFAULT_NATS_HOST, WADM_VERSION, WASMCLOUD_HOST_VERSION};
use crate::down::stop_nats;

use super::{dev_dir, sessions_file_path, SESSIONS_FILE_VERSION, SESSION_ID_LEN};

/// Metadata related to a single `wash dev` session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WashDevSession {
    /// Session ID
    pub(crate) id: String,
    /// Absolute path to the directory in which `wash dev` was run
    pub(crate) project_path: PathBuf,
    /// Tuple containing data about the host, in particular the
    /// host ID and path to log file
    ///
    /// This value may start out empty, but is filled in when a host is started
    pub(crate) host_data: Option<(String, PathBuf)>,
    /// Whether this session is currently in use
    pub(crate) in_use: bool,
    /// When this session was created
    pub(crate) created_at: DateTime<Utc>,
    /// When the wash dev session was last used
    pub(crate) last_used_at: DateTime<Utc>,
}

/// The structure of an a file containing sessions of `wash dev`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Version of the sessions sessions file
    pub(crate) version: Version,
    /// Sessions of `wash dev` that have been run at some point
    pub(crate) sessions: Vec<WashDevSession>,
}

impl SessionMetadata {
    /// Get the session file
    pub(crate) async fn open_sessions_file() -> Result<std::fs::File> {
        let sessions_file_path = sessions_file_path().await?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(false)
            .truncate(false)
            .open(&sessions_file_path)
            .with_context(|| {
                format!(
                    "failed to open sessions file [{}]",
                    sessions_file_path.display()
                )
            })
    }

    /// Build metadata from default file on disk
    pub(crate) async fn from_sessions_file() -> Result<Self> {
        // Open and lock the sessions file
        let mut sessions_file = Self::open_sessions_file().await?;
        let mut lock = file_guard::lock(&mut sessions_file, file_guard::Lock::Exclusive, 0, 1)?;

        // Load session metadata, if present
        let file_size = (*lock)
            .metadata()
            .context("failed to get sessions file metadata")?
            .len();
        let session_metadata: Self = if file_size == 0 {
            Self::default()
        } else {
            let sessions_file_path = sessions_file_path().await?;
            tokio::task::block_in_place(move || {
                let mut file_contents = Vec::with_capacity(
                    usize::try_from(file_size).context("failed to convert file size to usize")?,
                );
                lock.read_to_end(&mut file_contents)
                    .context("failed to read file contents")?;
                serde_json::from_slice::<Self>(&file_contents).with_context(|| {
                    format!(
                        "failed to parse session metadata from file [{}]",
                        sessions_file_path.display(),
                    )
                })
            })
            .with_context(|| format!("failed to read session metadata ({file_size} bytes)"))?
        };
        Ok(session_metadata)
    }

    /// Persist a single session to the metadata file that is on disk
    pub(crate) async fn persist_session(session: &WashDevSession) -> Result<()> {
        // Lock the session file
        let sessions_file_path = sessions_file_path().await?;
        let mut sessions_file = Self::open_sessions_file().await?;
        let mut lock = file_guard::lock(&mut sessions_file, file_guard::Lock::Exclusive, 0, 1)?;

        // Read the session file and ensure that the content is exactly similar to what we have now
        let file_size = (*lock)
            .metadata()
            .context("failed to get sessions file metadata")?
            .len();
        let mut session_metadata = if file_size == 0 {
            Self::default()
        } else {
            tokio::task::block_in_place(|| {
                let mut file_contents = Vec::with_capacity(
                    usize::try_from(file_size).context("failed to convert file size to usize")?,
                );
                lock.read_to_end(&mut file_contents)
                    .context("failed to read file contents")?;
                serde_json::from_slice::<Self>(&file_contents).with_context(|| {
                    format!(
                        "failed to parse session metadata from file [{}]",
                        sessions_file_path.display(),
                    )
                })
            })
            .context("failed to read session metadata while modifying session")?
        };

        // Update the session that was present
        if let Some(s) = session_metadata
            .sessions
            .iter_mut()
            .find(|s| s.id == session.id)
        {
            *s = session.clone();
        }

        // Write current updated session metadata to file
        tokio::fs::write(
            sessions_file_path,
            &serde_json::to_vec_pretty(&session_metadata)
                .context("failed to write session metadata")?,
        )
        .await?;

        Ok(())
    }
}

impl Default for SessionMetadata {
    fn default() -> Self {
        Self {
            version: SESSIONS_FILE_VERSION,
            sessions: Vec::new(),
        }
    }
}

impl WashDevSession {
    /// Get the directory into which all related log files/ancillary data should be stored
    pub(crate) async fn base_dir(&self) -> Result<PathBuf> {
        let base_dir = dev_dir().await.map(|p| p.join(&self.id))?;
        if !tokio::fs::try_exists(&base_dir)
            .await
            .context("failed to check if dev dir exists")?
        {
            tokio::fs::create_dir_all(&base_dir)
                .await
                .with_context(|| format!("failed to create dir [{}]", base_dir.display()))?;
        }
        Ok(base_dir)
    }

    /// Retrieve or create a `wash dev` session from a file on disk containing [`SessionMetadata`]
    pub(crate) async fn from_sessions_file(project_path: impl AsRef<Path>) -> Result<Self> {
        let mut session_metadata = SessionMetadata::from_sessions_file()
            .await
            .context("failed to load session metadata")?;
        let project_path = project_path.as_ref();

        // Attempt to find an session with the given path, creating one if necessary
        let session = if let Some(existing_session) = session_metadata
            .sessions
            .iter()
            .find(|s| s.project_path == project_path && !s.in_use) { existing_session.clone() } else {
            let session = Self {
                id: rand::thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(SESSION_ID_LEN)
                    .map(char::from)
                    .collect(),
                project_path: project_path.into(),
                host_data: None,
                in_use: true,
                created_at: Utc::now(),
                last_used_at: Utc::now(),
            };
            session_metadata.sessions.push(session.clone());
            session
        };

        Ok(session)
    }

    /// Start a host for the given session, if one is not present. Providing a host ID will
    /// cause the session to attempt to connect to the specified host, rather than starting a
    /// new one
    pub(crate) async fn start_host(
        &mut self,
        mut wasmcloud_opts: WasmcloudOpts,
        nats_opts: NatsOpts,
        wadm_opts: WadmOpts,
        host_id: Option<ServerId>,
    ) -> Result<(Option<Child>, Option<Child>, Option<Child>)> {
        if self.host_data.is_some() {
            return Ok((None, None, None));
        }

        eprintln!(
            "{} {}",
            emoji::CONSTRUCTION_BARRIER,
            style("Starting a new host...").bold()
        );
        // Ensure that file loads are allowed
        wasmcloud_opts.allow_file_load = Some(true);
        wasmcloud_opts.multi_local = true;

        let session_dir = self.base_dir().await?;

        let install_dir = downloads_dir()?;
        let nats_host = nats_opts.nats_host.clone().unwrap_or_else(|| {
            wasmcloud_opts
                .ctl_host
                .clone()
                .unwrap_or_else(|| DEFAULT_NATS_HOST.into())
        });
        let nats_port = nats_opts
            .nats_port
            .unwrap_or(wasmcloud_opts.ctl_port.unwrap_or(4222));
        let nats_listen_address = format!("{nats_host}:{nats_port}");

        let nats_child = if nats_opts.connect_only {
            None
        } else {
            // Start NATS
            let nats_log_path = session_dir.join("nats.log");
            let nats_binary = ensure_nats_server(&nats_opts.nats_version, &install_dir).await?;
            let nats_config = NatsConfig {
                host: nats_host,
                port: nats_port,
                store_dir: std::env::temp_dir().join(format!("wash-jetstream-{nats_port}")),
                js_domain: nats_opts.nats_js_domain,
                remote_url: nats_opts.nats_remote_url,
                credentials: nats_opts.nats_credsfile.clone(),
                websocket_port: nats_opts.nats_websocket_port,
                config_path: nats_opts.nats_configfile,
            };
            match start_nats(
                &install_dir,
                &nats_binary,
                nats_config,
                &nats_log_path,
                CommandGroupUsage::CreateNew,
            )
            .await
            {
                Ok(c) => Some(c),
                Err(e) if e.to_string().contains("already listening") => None,
                Err(e) => bail!("failed to start NATS server for wash dev: {e}"),
            }
        };

        // Start WADM
        let wadm_log_path = session_dir.join("wadm.log");
        let config = WadmConfig {
            structured_logging: wasmcloud_opts.enable_structured_logging,
            js_domain: wadm_opts.wadm_js_domain.clone(),
            nats_server_url: nats_listen_address,
            nats_credsfile: nats_opts.nats_credsfile,
        };
        let wadm_log_file = tokio::fs::File::create(&wadm_log_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create wadm log file @ [{}]",
                    wadm_log_path.display()
                )
            })?;
        let wadm_version = wadm_opts
            .wadm_version
            .unwrap_or_else(|| WADM_VERSION.into());
        let wadm_binary = ensure_wadm(&wadm_version, &install_dir).await?;
        let wadm_child = match start_wadm(
            &install_dir,
            &wadm_binary,
            wadm_log_file.into_std().await,
            Some(config),
            CommandGroupUsage::CreateNew,
        )
        .await
        {
            Ok(c) => Some(c),
            Err(e) => bail!("failed to start wadm for wash dev: {e}"),
        };

        // Start the host in detached mode, w/ custom log file
        let wasmcloud_version = Version::parse(
            wasmcloud_opts
                .clone()
                .wasmcloud_version
                .unwrap_or_else(|| WASMCLOUD_HOST_VERSION.into())
                .trim_start_matches('v'),
        )
        .context("parsing semantic wasmcloud version")?;
        let wasmcloud_log_path = session_dir.join("wasmcloud.log");
        let wasmcloud_binary = ensure_wasmcloud(&wasmcloud_version, &install_dir).await?;
        let log_output: Stdio = tokio::fs::File::create(&wasmcloud_log_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create log file @ [{}]",
                    wasmcloud_log_path.display()
                )
            })?
            .into_std()
            .await
            .into();
        let host_env = configure_host_env(wasmcloud_opts.clone()).await?;

        let (wasmcloud_child, host_id) = if let Some(host_id) = host_id {
            eprintln!(
                "{} {}",
                emoji::GREEN_CHECK,
                style(format!(
                    "Connected to host [{host_id}], refer to existing logs for details"
                ))
                .bold()
            );
            (None, host_id.to_string())
        } else {
            let wasmcloud_child = match start_wasmcloud_host(
                &wasmcloud_binary,
                std::process::Stdio::null(),
                log_output,
                host_env,
            )
            .await
            {
                Ok(child) => Some(child),
                Err(e) => {
                    eprintln!("{} Failed to start wasmCloud instance", emoji::ERROR);
                    if let Some(mut wadm) = wadm_child {
                        wadm.kill()
                            .await
                            .context("failed to stop wadm child process")?;
                        remove_wadm_pidfile(session_dir)
                            .await
                            .context("failed to remove wadm pidfile")?;
                    }
                    let nats_bin = install_dir.join(NATS_SERVER_BINARY);
                    let _ = stop_nats(install_dir, nats_bin).await?;
                    bail!("failed to start wasmCloud instance: {e}");
                }
            };

            // Read the log until we get output that
            let _wasmcloud_log_path = wasmcloud_log_path.clone();
            let host_id = tokio::time::timeout(
                tokio::time::Duration::from_secs(1),
                get_host_id(_wasmcloud_log_path),
            )
            .await
            .context("timeout expired while reading for Host ID in logs")?
            .context("failed to retrieve host ID from logs")?;

            eprintln!(
                "{} {}",
                emoji::GREEN_CHECK,
                style(format!(
                    "Successfully started host, logs writing to {}",
                    wasmcloud_log_path.display()
                ))
                .bold()
            );

            (wasmcloud_child, host_id)
        };

        self.host_data = Some((host_id, wasmcloud_log_path));

        Ok((nats_child, wadm_child, wasmcloud_child))
    }
}

async fn get_host_id(log_path: PathBuf) -> anyhow::Result<String> {
    let log_file = tokio::fs::File::open(&log_path)
        .await
        .with_context(|| format!("failed to open log file @ [{}]", &log_path.display()))?;

    // looks for the two variations of the log line containing the host_id:
    //   JSON: "host_id":"ABC123"
    //   LOG:  host_id="ABC123"
    let re = regex::Regex::new(r#"(?:\"host_id\":\s?\"|host_id=\")([A-Z0-9]+)\""#)
        .context("failed to compile regex")?;

    let mut lines = tokio::io::BufReader::new(log_file).lines();
    loop {
        if let Some(line) = lines
            .next_line()
            .await
            .context("failed to read line from file")?
        {
            // if there's no captures, this line doesn't contain the host_id, keep looking
            if let Some(captures) = re.captures(&line) {
                return Ok(captures
                    .get(1)
                    .context("failed to get capture group")?
                    .as_str()
                    .to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_get_host_id_from_standard_logging_pattern() {
        let log_path = NamedTempFile::new().unwrap().path().to_path_buf();
        tokio::fs::write(&log_path, "2024-12-13T17:17:07.287574Z  INFO wasmcloud_host::wasmbus: wasmCloud host started host_id=\"ABC123\"").await.unwrap();
        let host_id = get_host_id(log_path.clone()).await.unwrap();
        assert_eq!(host_id, "ABC123");
    }

    #[tokio::test]
    async fn test_get_host_id_from_structured_logging_pattern() {
        let log_path = NamedTempFile::new().unwrap().path().to_path_buf();
        tokio::fs::write(&log_path, "{{\"timestamp\":\"2024-12-12T01:43:53.749961Z\",\"level\":\"INFO\",\"fields\":{{\"message\":\"wasmCloud host started\",\"host_id\":\"DEF456\"}},\"target\":\"wasmcloud_host::wasmbus\",\"span\":{{\"name\":\"new\"}},\"spans\":[{{\"name\":\"new\"}}]}}").await.unwrap();
        let host_id = get_host_id(log_path.clone()).await.unwrap();
        assert_eq!(host_id, "DEF456");
    }
}
