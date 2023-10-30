use anyhow::{anyhow, bail, Result};
use std::{path::PathBuf, process::Stdio};
use tokio::process::Command;

pub struct CloneTemplate {
    /// temp folder where project will be cloned - deleted after 'wash new' completes
    pub clone_tmp: PathBuf,
    /// github repository URL, e.g., "https://github.com/wasmcloud/project-templates".
    /// For convenience, either prefix 'https://' or 'https://github.com' may be omitted.
    /// ssh urls may be used if ssh-config is setup appropriately.
    /// If a private repository is used, user will be prompted for credentials.
    pub repo_url: String,
    /// sub-folder of project template within the repo, e.g. "actor/hello"
    pub sub_folder: Option<String>,
    /// repo branch, e.g., main
    pub repo_branch: String,
}

pub async fn clone_git_template(opts: CloneTemplate) -> Result<()> {
    let cwd =
        std::env::current_dir().map_err(|e| anyhow!("could not get current directory: {}", e))?;
    std::env::set_current_dir(&opts.clone_tmp).map_err(|e| {
        anyhow!(
            "could not cd to tmp dir {}: {}",
            opts.clone_tmp.display(),
            e
        )
    })?;
    // for convenience, allow omission of prefix 'https://' or 'https://github.com'
    let repo_url = {
        if opts.repo_url.starts_with("http://") || opts.repo_url.starts_with("https://") {
            opts.repo_url
        } else if opts.repo_url.starts_with("github.com/") {
            format!("https://{}", &opts.repo_url)
        } else {
            format!(
                "https://github.com/{}",
                opts.repo_url.trim_start_matches('/')
            )
        }
    };

    let cmd_out = Command::new("git")
        .args(["clone", &repo_url, "--depth", "1", "--no-checkout", "."])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !cmd_out.status.success() {
        bail!(
            "git clone error: {}",
            String::from_utf8_lossy(&cmd_out.stderr)
        );
    }

    if let Some(sub_folder) = opts.sub_folder {
        let cmd_out = Command::new("git")
            .args(["sparse-checkout", "set", &sub_folder])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;
        if !cmd_out.status.success() {
            bail!(
                "git sparse-checkout set error: {}",
                String::from_utf8_lossy(&cmd_out.stderr)
            );
        }
    }

    let cmd_out = Command::new("git")
        .args(["checkout", &opts.repo_branch])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !cmd_out.status.success() {
        bail!(
            "git checkout error: {}",
            String::from_utf8_lossy(&cmd_out.stderr)
        );
    }
    std::env::set_current_dir(cwd)?;
    Ok(())
}

/// Information to find a specific commit in a Git repository.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitReference {
    /// From a tag.
    Tag(String),
    /// From a branch.
    Branch(String),
    /// From a specific revision.
    Rev(String),
    /// The default branch of the repository, the reference named `HEAD`.
    DefaultBranch,
}
