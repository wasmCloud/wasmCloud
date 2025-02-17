use anyhow::Error;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use semver::Version;
use serde::{Deserialize, Serialize};
use tracing::trace;

use super::get_download_client_with_user_agent;

type DateTimeUtc = DateTime<Utc>;

/// GitHub page max <https://docs.github.com/en/rest/releases/releases?apiVersion=2022-11-28#list-releases>
const GITHUB_PER_PAGE: u32 = 100;

// Batch size to fetch releases from GitHub
const GITHUB_REQUEST_BATCH_SIZE: u32 = 30;

const VERSION_FETCHER_CLIENT_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// `get_chronologically_sorted_releases` returns with a list of chronologically ordered (latest first)
/// releases that are more recent than the provided `after_version`.
async fn get_chronologically_sorted_releases(
    owner: &str,
    repo: &str,
    after_version: &semver::Version,
) -> Result<Vec<GitHubRelease>, anyhow::Error> {
    let releases_of_repo = fetch_latest_releases(owner, repo, after_version).await?;
    let mut releases_of_repo = releases_of_repo.into_iter().collect::<Vec<GitHubRelease>>();
    releases_of_repo.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    Ok(releases_of_repo)
}

/// Get a full list of github patch releases that exist after the provided version.
pub async fn new_patch_releases_after(
    owner: &str,
    repo: &str,
    after_version: &Version,
) -> Result<Vec<GitHubRelease>, Error> {
    let releases = get_chronologically_sorted_releases(owner, repo, after_version).await?;
    let main_releases = releases
        .into_iter()
        .filter(|release| match &release.get_main_artifact_release() {
            Some(version) => {
                after_version.major == version.major && after_version.minor == version.minor
            }
            None => false,
        })
        .collect::<Vec<GitHubRelease>>();
    Ok(main_releases)
}

/// Returns the latest patch version of the provided version.
pub async fn new_patch_version_of_after_string(
    owner: &str,
    repo: &str,
    after_version: &str,
) -> Result<Option<Version>, Error> {
    let after_version = after_version.strip_prefix('v').unwrap_or(after_version);
    let after_version = match Version::parse(after_version) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    match new_patch_releases_after(owner, repo, &after_version).await {
        Ok(patches) => match patches.first() {
            Some(patch) => Ok(patch.get_main_artifact_release()),
            None => Ok(None),
        },
        _ => Ok(None),
    }
}

/// `GitHubRelease` represents the necessary fields to determine wadm and/or wasmCloud
/// GitHub release (<https://developer.github.com/v3/repos/releases/>) object
/// has new patch version available. The fields are based on the response from the
/// response schema from the docs.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: String,
    #[serde(with = "github_date_format")]
    pub published_at: DateTimeUtc,
    pub draft: bool,
    pub prerelease: bool,
}

impl PartialEq for GitHubRelease {
    fn eq(&self, other: &Self) -> bool {
        self.tag_name == other.tag_name
    }
}

impl GitHubRelease {
    #[must_use]
    pub fn get_main_artifact_release(&self) -> Option<semver::Version> {
        match self.tag_name.strip_prefix('v') {
            Some(v) => match semver::Version::parse(v) {
                Ok(v) => {
                    if self.draft || self.prerelease {
                        None
                    } else {
                        Some(v)
                    }
                }
                Err(_) => None,
            },
            None => None,
        }
    }
}

/// Returns the URL to fetch the latest release from the GitHub repository.
/// doc: <https://developer.github.com/v3/repos/releases/#get-the-latest-release>
fn format_latest_releases_url(owner: &str, repo: &str, page: u32) -> String {
    format!(
        "https://api.github.com/repos/{owner}/{repo}/releases?page={page}&per_page={GITHUB_PER_PAGE}"
    )
}

async fn fetch_latest_releases(
    owner: &str,
    repo: &str,
    latest_interested: &semver::Version,
) -> Result<Vec<GitHubRelease>, anyhow::Error> {
    let client = get_download_client_with_user_agent(VERSION_FETCHER_CLIENT_USER_AGENT)?;
    let mut page = 0u32;
    let mut releases: Vec<GitHubRelease> = Vec::new();
    'fetch_loop: loop {
        trace!(
            "Fetching releases from GitHub starting at page: {} with batch {}",
            page,
            GITHUB_REQUEST_BATCH_SIZE
        );
        let batchreleases =
            get_release_batch(page, GITHUB_REQUEST_BATCH_SIZE, owner, repo, client.clone()).await?;
        for release in &batchreleases {
            if let Some(main_release) = release.get_main_artifact_release() {
                if main_release == *latest_interested {
                    break 'fetch_loop;
                }
            }
            releases.push(release.clone());
        }
        if batchreleases.is_empty() {
            break 'fetch_loop;
        }
        page += GITHUB_REQUEST_BATCH_SIZE;
    }
    Ok(releases)
}

/// Helper function to fetch a batch of releases from GitHub
/// instead of linearly fetching one page at a time.
/// Since we do not know the total number of releases, we fetch
/// in batches of `GITHUB_REQUEST_BATCH_SIZE` pages, to avoid over-fetching
/// with parallel requests to exhaust the rate limit with this repository.
/// If the release we are interested in is found, we stop fetching, otherwise
/// we continue fetching until we reach the end of the releases or due to the rate limit no more responses can be fetched.
///
/// # Arguments
/// * `current_page` is the current page to start fetching from
/// * `batch_size` is the number of requests to fetch in parallel
/// * `owner` is the owner of the repository
/// * `repo` is the repository name
/// * `client` is the reqwest client to use for fetching
async fn get_release_batch(
    current_page: u32,
    batch_size: u32,
    owner: &str,
    repo: &str,
    client: reqwest::Client,
) -> Result<Vec<GitHubRelease>, reqwest::Error> {
    let mut tasks = Vec::new();
    for page in current_page..=current_page + batch_size {
        let url = format_latest_releases_url(owner, repo, page);

        let client_clone = client.clone();
        tasks.push(async move {
            let response = client_clone.get(&url).send().await?;
            if !response.status().is_success() {
                return Err(anyhow::Error::msg(format!(
                    "Failed to fetch releases from GitHub at url: {} with status {}",
                    url,
                    response.status()
                )));
            }
            let releases_on_page = response.json::<Vec<GitHubRelease>>().await?;
            Ok(releases_on_page)
        });
    }
    let results = join_all(tasks).await;
    Ok(results.into_iter().flatten().flatten().collect())
}

/// Custom serde implementation for GitHub date format (`YYYY-MM-DDTHH:MM:SSZ ISO 8601`).
mod github_date_format {
    use chrono::{DateTime, NaiveDateTime, Utc};
    use serde::{self, Deserialize, Deserializer, Serializer};

    const FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

    pub fn serialize<S>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}", date.format(FORMAT));
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let dt = NaiveDateTime::parse_from_str(&s, FORMAT).map_err(serde::de::Error::custom)?;
        Ok(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    /// Test if the `GitHubRelease` struct is parsed correctly from the raw string.
    /// Removed some items from the raw string to keep the test readable.
    #[test]
    fn test_github_release_is_parsed_correctly() {
        let raw_string = r#"
        {
            "url": "https://api.github.com/repos/wasmCloud/wasmCloud/releases/165886656",
            "assets_url": "https://api.github.com/repos/wasmCloud/wasmCloud/releases/165886656/assets",
            "upload_url": "https://uploads.github.com/repos/wasmCloud/wasmCloud/releases/165886656/assets{?name,label}",
            "html_url": "https://github.com/wasmCloud/wasmCloud/releases/tag/washboard-ui-v0.4.0",
            "id": 165886656,
            "node_id": "RE_kwDOEiTU7M4J4zrA",
            "tag_name": "washboard-ui-v0.4.0",
            "target_commitish": "main",
            "name": "washboard-ui-v0.4.0",
            "draft": false,
            "prerelease": false,
            "created_at": "2024-07-17T14:47:54Z",
            "published_at": "2024-07-17T16:15:15Z",
            "tarball_url": "https://api.github.com/repos/wasmCloud/wasmCloud/tarball/washboard-ui-v0.4.0",
            "zipball_url": "https://api.github.com/repos/wasmCloud/wasmCloud/zipball/washboard-ui-v0.4.0",
            "mentions_count": 5
        }
        "#;

        let release = serde_json::from_str::<GitHubRelease>(raw_string);
        assert!(release.is_ok());
        let release = release.unwrap();
        assert_eq!(release.tag_name, "washboard-ui-v0.4.0");
        assert_eq!(release.name, "washboard-ui-v0.4.0");

        let expected_date = NaiveDate::from_ymd_opt(2024, 0o7, 17)
            .unwrap()
            .and_hms_opt(16, 15, 15)
            .unwrap()
            .and_utc();
        assert_eq!(release.published_at, expected_date);
        assert!(!release.draft);
        assert!(!release.prerelease);
    }

    #[test]
    fn test_github_release_is_not_draft_or_pre_release() {
        let release = GitHubRelease {
            tag_name: "v0.4.0".to_string(),
            name: "v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        assert!(release.get_main_artifact_release().is_some());
    }

    #[test]
    fn test_semver_without_prefix() {
        let release = GitHubRelease {
            tag_name: "v0.4.0".to_string(),
            name: "v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        let version = release.get_main_artifact_release();
        assert!(version.is_some());
        assert_eq!(version.unwrap(), semver::Version::parse("0.4.0").unwrap());

        let release_with_prefix = GitHubRelease {
            tag_name: "washboard-ui-v0.4.0".to_string(),
            name: "washboard-ui-v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        let version = release_with_prefix.get_main_artifact_release();
        assert!(version.is_none());
    }
}
