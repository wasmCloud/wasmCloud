use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use regex::Regex;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use tracing::debug;

use super::{
    get_download_client_with_user_agent, GITHUB_WASMCLOUD_ORG, GITHUB_WASMCLOUD_WASMCLOUD_REPO,
};

type DateTimeUtc = DateTime<Utc>;

/// GitHub page max <https://docs.github.com/en/rest/releases/releases?apiVersion=2022-11-28#list-releases>
const GITHUB_PER_PAGE: u32 = 100;

/// Batch size to fetch releases from GitHub. Max for unauthenticated requests is *60 per hour*. If we start to hit this
/// rate limit, we might need to find an alternative solution for interacting with the GitHub API.
const GITHUB_REQUEST_BATCH_SIZE: u32 = 3;

const VERSION_FETCHER_CLIENT_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Gets a list of releases that are more newer, but semver compatible with the provided `after_version`. They are
/// sorted by the version number, and the published date. Optionally, a tag pattern can be provided to filter the
/// releases. If this is not provided, only the main releases are considered.
async fn get_sorted_releases(
    owner: &str,
    repo: &str,
    after_version: &Version,
    tag_pattern: Option<&str>,
) -> Result<Vec<GitHubRelease>, anyhow::Error> {
    let releases_of_repo = fetch_latest_releases(owner, repo, after_version, tag_pattern).await?;
    let mut releases_of_repo = releases_of_repo.into_iter().collect::<Vec<GitHubRelease>>();
    releases_of_repo.sort_by(|a, b| b.cmp(a));
    Ok(releases_of_repo)
}

pub async fn get_wash_versions_newer_than(
    after_version: &Version,
) -> Result<Vec<GitHubRelease>, anyhow::Error> {
    const TAG_PATTERN: &str = "wash-v.*";
    get_sorted_releases(
        GITHUB_WASMCLOUD_ORG,
        GITHUB_WASMCLOUD_WASMCLOUD_REPO,
        after_version,
        Some(TAG_PATTERN),
    )
    .await
}

pub async fn new_patch_releases_after(
    owner: &str,
    repo: &str,
    after_version: &Version,
) -> Result<Vec<GitHubRelease>, Error> {
    new_releases_after(owner, repo, None, after_version, Compare::Patch).await
}

pub async fn new_minor_releases_after(
    owner: &str,
    repo: &str,
    after_version: &Version,
) -> Result<Vec<GitHubRelease>, Error> {
    new_releases_after(owner, repo, None, after_version, Compare::Compatible).await
}

pub async fn new_tag_pattern_patch_releases_after(
    owner: &str,
    repo: &str,
    after_version: &Version,
    tag_pattern: Option<&str>,
) -> Result<Vec<GitHubRelease>, Error> {
    new_releases_after(owner, repo, tag_pattern, after_version, Compare::Patch).await
}

pub async fn new_tag_pattern_compatible_releases_after(
    owner: &str,
    repo: &str,
    after_version: &Version,
    tag_pattern: Option<&str>,
) -> Result<Vec<GitHubRelease>, Error> {
    new_releases_after(owner, repo, tag_pattern, after_version, Compare::Compatible).await
}

enum Compare {
    /// Compatible version `^`
    Compatible,
    /// Patch version `~`
    Patch,
}
impl Compare {
    fn as_str(&self) -> &str {
        match self {
            Compare::Compatible => "^",
            Compare::Patch => "~",
        }
    }
}

/// Get a full list of releases from the GitHub repository that are newer than the provided version.
/// Optionally, a tag pattern can be provided to filter the releases. If this is not provided, only the main releases
/// are considered.
async fn new_releases_after(
    owner: &str,
    repo: &str,
    tag_pattern: Option<&str>,
    after_version: &Version,
    comparator: Compare,
) -> Result<Vec<GitHubRelease>, Error> {
    let releases = get_sorted_releases(owner, repo, after_version, tag_pattern).await?;
    let op = comparator.as_str();
    let main_releases = releases
        .into_iter()
        .filter(|release| {
            release.satisfies_constraint(&format!("{op}{v}, >{v}", v = after_version))
        })
        .collect::<Vec<GitHubRelease>>();
    Ok(main_releases)
}

/// Get newest patch or pre-1.0.0 minor version after the provided version string. Optionally provide a tag pattern
/// to filter the releases. If this is not provided, only the main releases are considered.
///
/// Note: In pre-1.0.0 releases, the [`semver` spec](https://semver.org/#spec-item-4) treats the third number (0.0.X) as
/// the minor. We're ok with the minor version changing in pre-1.0.0 releases of the tools we're downloading right now.
/// This may change in future, and if it does, this function name should change.
pub async fn new_patch_or_pre_1_0_0_minor_version_after_version_string(
    owner: &str,
    repo: &str,
    version_string: &str,
    tag_pattern: Option<&str>,
) -> Result<Version, Error> {
    let version_string = version_string.strip_prefix('v').unwrap_or(version_string);
    let version = Version::parse(version_string).expect("failed to parse version");
    // if the version is pre-1.0.0, we need to use the semver compatible version (^) instead of the patch version (~)
    let comparator = match version.major == 0 {
        true => Compare::Compatible,
        false => Compare::Patch,
    };
    let releases = new_releases_after(owner, repo, tag_pattern, &version, comparator)
        .await
        .expect("failed to fetch releases");
    match releases.first() {
        Some(release) => release.get_x_y_z_version(),
        _ => Err(anyhow::Error::msg("No new semver compatible version found")),
    }
}

pub fn parse_version_string(version: Option<String>) -> Option<Version> {
    match version {
        Some(version) => {
            debug!("Using specified version: {version}");
            Some(
                Version::parse(version.trim_start_matches('v')).unwrap_or_else(|_| {
                    panic!(r"Invalid version '{version}'. Expected semantic version (v0.1.0)");
                }),
            )
        }
        None => {
            debug!("No version specified, using default version");
            None
        }
    }
}

/// A GitHub release object from the GitHub API with some helper methods
///
/// [`GitHubRelease`] represents the a subset of fields from a [GitHub Release API Response](https://developer.github.com/v3/repos/releases/)
/// that are necessary to determine if a release of a tool (wadm, wasmCloud, NATS, etc.) has new version available.
///
/// It also provides some helper methods to check assist with these checks.
#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: String,
    #[serde(with = "github_date_format")]
    pub published_at: DateTimeUtc,
    pub draft: bool,
    pub prerelease: bool,
}

impl Ord for GitHubRelease {
    /// Sorts by ascending version number, and then by earliest published date.
    fn cmp(&self, other: &Self) -> Ordering {
        let self_version = self.get_x_y_z_version().unwrap();
        let other_version = other.get_x_y_z_version().unwrap();

        Ord::cmp(
            &(&self_version, &self.published_at),
            &(&other_version, &other.published_at),
        )
    }
}

impl PartialOrd for GitHubRelease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl GitHubRelease {
    /// Returns the version of the release if is a "main artifact" release and is not a draft or prerelease. We consider
    /// a "main artifact" release to be a release that only has version string as the tag name (i.e. `v1.4.0`).
    pub fn get_main_artifact_release(&self) -> Option<Version> {
        // This pattern is simple, but we're only using it as an initial check. The actual version is extracted by
        // by `semver::Version`.
        self.get_published_version(r"^v\d\.\d+\.\d+")
    }

    /// Returns the version of the release if matches the tag pattern and is not a draft or prerelease.
    pub fn get_tagged_artifact_release(&self, tag_pattern: &str) -> Option<Version> {
        self.get_published_version(tag_pattern)
    }

    /// Returns the version of the release by parsing the tag name. If the tag name does not contain a valid semver
    /// version, it will return `None`.
    pub fn get_x_y_z_version(&self) -> Result<Version, Error> {
        let re = Regex::new(r"\d+\.\d+\.\d+").unwrap();
        let version = re
            .find(self.tag_name.as_str())
            .unwrap_or_else(|| panic!("failed to find x.y.z version in tag: {}", self.tag_name))
            .as_str();
        Version::parse(version).context("failed to parse version")
    }

    /// Returns `true` if the version passes a check using the constraint string when tested through [`VersionReq::parse`],
    pub fn satisfies_constraint(&self, constraint_string: &str) -> bool {
        let req = VersionReq::parse(constraint_string).unwrap_or_default();
        let version = self.get_x_y_z_version().unwrap();
        debug!(
            "Checking if version {} satisfies constraint {}",
            version, constraint_string
        );

        req.matches(&version)
    }

    /// Returns the version of the release if it is not a draft or prerelease that match the tag pattern.
    fn get_published_version(&self, tag_pattern: &str) -> Option<Version> {
        if self.draft || self.prerelease {
            return None;
        }

        if self.matches_tag_pattern(tag_pattern) {
            self.get_x_y_z_version().ok()
        } else {
            None
        }
    }

    /// Returns `true` if the tag name matches a [`Regex`] created from the provided `tag_pattern`.
    fn matches_tag_pattern(&self, tag_pattern: &str) -> bool {
        let re = Regex::new(tag_pattern)
            .unwrap_or_else(|_| panic!("failed to create regex from tag pattern: {tag_pattern}"));
        re.is_match(self.tag_name.as_str())
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
    latest_interested: &Version,
    tag_pattern: Option<&str>,
) -> Result<Vec<GitHubRelease>, anyhow::Error> {
    let client = get_download_client_with_user_agent(VERSION_FETCHER_CLIENT_USER_AGENT)?;
    let mut page = 1u32; // GitHub page starts from 1
    let mut releases: Vec<GitHubRelease> = Vec::new();
    'fetch_loop: loop {
        debug!(
            "Fetching releases from GitHub starting at page: {} with batch {}",
            page, GITHUB_REQUEST_BATCH_SIZE
        );
        let batchreleases =
            get_release_batch(page, GITHUB_REQUEST_BATCH_SIZE, owner, repo, client.clone()).await?;
        if batchreleases.is_empty() {
            break 'fetch_loop;
        }
        for release in &batchreleases {
            if let Some(version) = match tag_pattern {
                Some(tag_pattern) => release.get_tagged_artifact_release(tag_pattern),
                None => release.get_main_artifact_release(),
            } {
                if version == *latest_interested {
                    break 'fetch_loop;
                }
                releases.push(release.clone());
            }
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
        assert_eq!(version.unwrap(), Version::parse("0.4.0").unwrap());

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

    #[test]
    fn test_semver_with_prefix() {
        let release = GitHubRelease {
            tag_name: "washboard-ui@v0.4.0".to_string(),
            name: "washboard-ui@v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        let version = release.get_tagged_artifact_release("washboard-ui");
        assert!(version.is_some());
        assert_eq!(version.unwrap(), Version::parse("0.4.0").unwrap());
    }

    #[test]
    fn test_github_release_is_not_draft_or_pre_release_with_tag_pattern() {
        let release = GitHubRelease {
            tag_name: "washboard-ui@v0.4.0".to_string(),
            name: "washboard-ui@v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        assert!(release
            .get_tagged_artifact_release("washboard-ui")
            .is_some());

        let release = GitHubRelease {
            tag_name: "washboard-ui@v0.4.0".to_string(),
            name: "washboard-ui@v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: true,
        };
        assert!(release
            .get_tagged_artifact_release("washboard-ui")
            .is_none());

        let release = GitHubRelease {
            tag_name: "washboard-ui@v0.4.0".to_string(),
            name: "washboard-ui@v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: true,
            prerelease: false,
        };
        assert!(release
            .get_tagged_artifact_release("washboard-ui")
            .is_none());
    }

    #[test]
    fn test_github_release_is_newer() {
        let release = GitHubRelease {
            tag_name: "v0.4.0".to_string(),
            name: "v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        assert!(release.satisfies_constraint("^0.4.0"));
        assert!(release.satisfies_constraint(">0.3.0"));
        assert!(!release.satisfies_constraint("^0.5.0"));
    }

    #[test]
    fn test_github_release_is_newer_with_tag_pattern() {
        let release = GitHubRelease {
            tag_name: "washboard-ui@v0.4.0".to_string(),
            name: "washboard-ui@v0.4.0".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };
        assert!(release.satisfies_constraint("^0.4.0"));
        assert!(release.satisfies_constraint(">0.3.0"));
        assert!(!release.satisfies_constraint("^0.5.0"));
    }

    #[test]
    fn test_github_release_is_newer_with_tag_pattern_major() {
        let release = GitHubRelease {
            tag_name: "v1.7.10".to_string(),
            name: "v1.7.10".to_string(),
            published_at: Utc::now(),
            draft: false,
            prerelease: false,
        };

        assert!(release.satisfies_constraint("^1.7.5"));
        assert!(release.satisfies_constraint("^1.7.0"));
        assert!(!release.satisfies_constraint("^0.99.0"));
        assert!(!release.satisfies_constraint("^2.5.0"));
    }
}
