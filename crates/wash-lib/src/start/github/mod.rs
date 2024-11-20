//! Reusable code for downloading tarballs from GitHub releases

use anyhow::{anyhow, bail, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{ffi::OsStr, io::Cursor};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
use wasmcloud_core::tls::NativeRootsExt;

const DOWNLOAD_CLIENT_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub const GITHUB_WASMCLOUD_ORG: &str = "wasmCloud";
pub const GITHUB_WASMCLOUD_WASMCLOUD_REPO: &str = "wasmCloud";
pub const GITHUB_WASMCLOUD_WADM_REPO: &str = "wadm";

/// Reusable function to download a release tarball from GitHub and extract an embedded binary to a specified directory
///
/// # Arguments
///
/// * `url` - URL of the GitHub release artifact tarball (Usually in the form of https://github.com/<owner>/<repo>/releases/download/<tag>/<artifact>.tar.gz)
/// * `dir` - Directory on disk to install the binary into. This will be created if it doesn't exist
/// * `bin_name` - Name of the binary inside of the tarball, e.g. `nats-server` or `wadm`
/// # Examples
///
/// ```rust,ignore
/// # #[tokio::main]
/// # async fn main() {
/// let url = "https://github.com/wasmCloud/wadm/releases/download/v0.4.0-alpha.1/wadm-v0.4.0-alpha.1-linux-amd64.tar.gz";
/// let res = download_binary_from_github(url, "/tmp/", "wadm").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wadm");
/// # }
/// ```
pub async fn download_binary_from_github<P>(url: &str, dir: P, bin_name: &str) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let bin_path = dir.as_ref().join(bin_name);
    // Download release tarball
    let body = match get_download_client()?.get(url).send().await {
        Ok(resp) => resp.bytes().await?,
        Err(e) => bail!("Failed to request release tarball: {:?}", e),
    };
    let cursor = Cursor::new(body);
    let mut bin_tarball = Archive::new(Box::new(GzipDecoder::new(cursor)));

    // Look for binary within tarball and only extract that
    let mut entries = bin_tarball.entries()?;
    while let Some(res) = entries.next().await {
        let mut entry = res.map_err(|e| {
            anyhow!(
                "Failed to retrieve file from archive, ensure {bin_name} exists. Original error: {e}",
            )
        })?;
        if let Ok(tar_path) = entry.path() {
            match tar_path.file_name() {
                Some(name) if name == OsStr::new(bin_name) => {
                    // Ensure target directory exists
                    create_dir_all(&dir).await?;
                    let mut bin_file = File::create(&bin_path).await?;
                    // Make binary executable
                    #[cfg(target_family = "unix")]
                    {
                        let mut permissions = bin_file.metadata().await?.permissions();
                        // Read/write/execute for owner and read/execute for others. This is what `cargo install` does
                        permissions.set_mode(0o755);
                        bin_file.set_permissions(permissions).await?;
                    }

                    tokio::io::copy(&mut entry, &mut bin_file).await?;
                    return Ok(bin_path);
                }
                // Ignore all other files in the tarball
                _ => (),
            }
        }
    }

    bail!("{bin_name} binary could not be installed, please see logs")
}

/// Helper function to determine if the provided binary is present in a directory
#[allow(unused)]
pub(crate) async fn is_bin_installed<P>(dir: P, bin_name: &str) -> bool
where
    P: AsRef<Path>,
{
    metadata(dir.as_ref().join(bin_name))
        .await
        .map_or(false, |m| m.is_file())
}

/// Helper function to set up a reqwest client for performing the download
pub(crate) fn get_download_client() -> Result<reqwest::Client> {
    get_download_client_with_user_agent(DOWNLOAD_CLIENT_USER_AGENT)
}

/// Helper function to set up a reqwest client for performing the download with a user agent
pub(crate) fn get_download_client_with_user_agent(user_agent: &str) -> Result<reqwest::Client> {
    let proxy_username = std::env::var("WASH_PROXY_USERNAME").unwrap_or_default();
    let proxy_password = std::env::var("WASH_PROXY_PASSWORD").unwrap_or_default();

    let mut builder = reqwest::ClientBuilder::default()
        .user_agent(user_agent)
        .with_native_certificates();

    if let Ok(http_proxy) = std::env::var("HTTP_PROXY").or_else(|_| std::env::var("http_proxy")) {
        let mut proxy = reqwest::Proxy::http(http_proxy)?.no_proxy(reqwest::NoProxy::from_env());
        if !proxy_username.is_empty() && !proxy_password.is_empty() {
            proxy = proxy.basic_auth(&proxy_username, &proxy_password);
        }
        builder = builder.proxy(proxy);
    }

    if let Ok(https_proxy) = std::env::var("HTTPS_PROXY").or_else(|_| std::env::var("https_proxy"))
    {
        let mut proxy = reqwest::Proxy::https(https_proxy)?.no_proxy(reqwest::NoProxy::from_env());
        if !proxy_username.is_empty() && !proxy_password.is_empty() {
            proxy = proxy.basic_auth(&proxy_username, &proxy_password);
        }
        builder = builder.proxy(proxy);
    }

    Ok(builder.build()?)
}

mod api;
pub use api::*;

#[cfg(test)]
#[cfg(target_os = "linux")]
// NOTE: These are only run on linux for CI purposes, because they rely on the docker client being
// available, and for various reasons this has proven to be problematic on both the Windows and
// MacOS runners we use.
mod test {
    use std::{collections::HashMap, env::temp_dir};

    use tokio::fs::{create_dir_all, remove_dir_all};
    use tokio::io::AsyncBufReadExt;
    use wasmcloud_test_util::testcontainers::{AsyncRunner as _, ImageExt, Mount, SquidProxy};

    use crate::start::{get_download_client, github::DOWNLOAD_CLIENT_USER_AGENT};

    // For squid config reference, see: https://www.squid-cache.org/Doc/config/
    // Sets up a squid-proxy listening on port 3128 that requires basic auth
    const SQUID_CONFIG_WITH_BASIC_AUTH: &str = r#"
# Listen on port 3128 for traffic, allows proxy to run as http endpoint,
# while still serving responses for both HTTP_PROXY and HTTPS_PROXY.
http_port 3128
# log to stdout to make the logs accessible
logfile_rotate 0
# This format translates to: <request-method>|<url>|<return-code>|<user-agent>|<basic-auth-username>
logformat wasmcloud %rm|%ru|%>Hs|%{User-Agent}>h|%[un
cache_log stdio:/dev/stdout
access_log stdio:/dev/stderr wasmcloud
cache_store_log stdio:/dev/stdout
# This set of directives tells squid to require basic auth,
# but the passed in credentials can be whatever to make testing easier.
auth_param basic program /usr/libexec/basic_fake_auth
acl authenticated proxy_auth REQUIRED
http_access allow authenticated
http_access deny all
shutdown_lifetime 1 seconds
"#;

    // Sets up a squid-proxy listening on port 3128 that does not require any auth
    const SQUID_CONFIG_WITHOUT_AUTH: &str = r#"
http_port 3128
# log to stdout to make the logs accessible
logfile_rotate 0
logformat wasmcloud %rm|%ru|%>Hs|%{User-Agent}>h|%[un
cache_log stdio:/dev/stdout
access_log stdio:/dev/stderr wasmcloud
cache_store_log stdio:/dev/stdout
# Log query params
strip_query_terms off
# allow unauthenticated http(s) access
http_access allow all
shutdown_lifetime 1 seconds
"#;

    #[tokio::test]
    #[cfg_attr(not(docker_available), ignore = "docker isn't available")]
    async fn test_download_client_with_proxy_settings() {
        // NOTE: This is intentional to avoid the two tests running in parallel
        // and contaminating each other's environment variables for configuring
        // the http client based on the environment.
        test_http_proxy_without_auth().await;
        test_http_proxy_with_basic_auth().await;
    }

    async fn test_http_proxy_without_auth() {
        let dir_path = temp_dir().join("test_http_proxy_no_auth");
        let _ = remove_dir_all(&dir_path).await;
        create_dir_all(&dir_path).await.unwrap();

        let squid_config_path = dir_path.join("squid.conf");
        tokio::fs::write(squid_config_path.clone(), SQUID_CONFIG_WITHOUT_AUTH)
            .await
            .unwrap();

        let container = SquidProxy::default()
            .with_mount(Mount::bind_mount(
                squid_config_path.to_string_lossy().to_string(),
                "/etc/squid.conf",
            ))
            .start()
            .await
            .expect("failed to start squid-proxy container");

        let mut env_vars = HashMap::from([("HTTP_PROXY", None), ("HTTPS_PROXY", None)]);
        // Setup environment variables for the client
        for env_var in env_vars.clone().keys() {
            // Store the previous value so we can reset it once the test is done.
            if let Ok(value) = std::env::var(env_var) {
                env_vars.entry(env_var).and_modify(|v| *v = Some(value));
            }
            std::env::set_var(
                env_var,
                format!(
                    "http://localhost:{}",
                    container
                        .get_host_port_ipv4(3128)
                        .await
                        .expect("failed to get squid-proxy host port")
                ),
            );
        }

        let client = get_download_client().unwrap();
        let http_endpoint = "http://httpbin.org/get";
        let https_endpoint = "https://httpbin.org/get";
        let http = client.get(http_endpoint).send().await.unwrap();
        let https = client.get(https_endpoint).send().await.unwrap();

        let _ = container.stop().await;

        assert_eq!(http.status(), reqwest::StatusCode::OK);
        assert_eq!(https.status(), reqwest::StatusCode::OK);

        let mut stderr = vec![];
        let mut lines = container.stderr(false).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            stderr.push(line);
        }

        // GET|http://httpbin.org/get|200|wash-lib/0.21.1|-
        let http_log_entry = format!("GET|{http_endpoint}|200|{}|-", DOWNLOAD_CLIENT_USER_AGENT);
        assert!(stderr.contains(&http_log_entry));

        // CONNECT|httpbin.org:443|200|wash-lib/0.21.1|-
        let https_url = url::Url::parse(https_endpoint).unwrap();
        let https_log_entry = format!(
            "CONNECT|{}:{}|200|{}|-",
            https_url.host_str().unwrap(),
            https_url.port_or_known_default().unwrap(),
            DOWNLOAD_CLIENT_USER_AGENT
        );
        assert!(stderr.contains(&https_log_entry));

        // Restore the environment variables prior to the test run
        for (key, val) in env_vars {
            if let Some(value) = val {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }

        let _ = remove_dir_all(dir_path).await;
    }

    async fn test_http_proxy_with_basic_auth() {
        let dir_path = temp_dir().join("test_http_proxy_basic_auth");
        let _ = remove_dir_all(&dir_path).await;
        create_dir_all(&dir_path).await.unwrap();

        let squid_config_path = dir_path.join("squid.conf");
        tokio::fs::write(squid_config_path.clone(), SQUID_CONFIG_WITH_BASIC_AUTH)
            .await
            .unwrap();

        let container = SquidProxy::default()
            .with_mount(Mount::bind_mount(
                squid_config_path.to_string_lossy().to_string(),
                "/etc/squid.conf",
            ))
            .start()
            .await
            .expect("failed to start squid-proxy container");

        let mut env_vars = HashMap::from([("HTTP_PROXY", None), ("HTTPS_PROXY", None)]);
        // Setup environment variables for the client
        for env_var in env_vars.clone().keys() {
            // Store the previous value so we can reset it once the test is done.
            if let Ok(value) = std::env::var(env_var) {
                env_vars.entry(env_var).and_modify(|v| *v = Some(value));
            }
            std::env::set_var(
                env_var,
                format!(
                    "http://localhost:{}",
                    container
                        .get_host_port_ipv4(3128)
                        .await
                        .expect("failed to get squid-proxy port")
                ),
            );
        }
        let proxy_username = "wasmcloud";
        std::env::set_var("WASH_PROXY_USERNAME", proxy_username);
        std::env::set_var("WASH_PROXY_PASSWORD", "this-can-be-whatever");

        let client = get_download_client().unwrap();
        let http_endpoint = "http://httpbin.org/get";
        let https_endpoint = "https://httpbin.org/get";
        let http = client.get(http_endpoint).send().await.unwrap();
        let https = client.get(https_endpoint).send().await.unwrap();

        let _ = container.stop().await;

        assert_eq!(http.status(), reqwest::StatusCode::OK);
        assert_eq!(https.status(), reqwest::StatusCode::OK);

        let mut stderr = vec![];
        let mut lines = container.stderr(false).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            stderr.push(line);
        }

        // GET|http://httpbin.org/get|200|wash-lib/0.21.1|wasmcloud
        let http_log_entry = format!(
            "GET|{http_endpoint}|200|{}|{proxy_username}",
            DOWNLOAD_CLIENT_USER_AGENT
        );
        assert!(stderr.contains(&http_log_entry));

        // CONNECT|httpbin.org:443|200|wash-lib/0.21.1|wasmcloud
        let https_url = url::Url::parse(https_endpoint).unwrap();
        let https_log_entry = format!(
            "CONNECT|{}:{}|200|{}|{proxy_username}",
            https_url.host_str().unwrap(),
            https_url.port_or_known_default().unwrap(),
            DOWNLOAD_CLIENT_USER_AGENT
        );
        assert!(stderr.contains(&https_log_entry));

        // Restore the environment variables prior to the test run
        for (key, val) in env_vars {
            if let Some(value) = val {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }

        let _ = remove_dir_all(dir_path).await;
    }
}
