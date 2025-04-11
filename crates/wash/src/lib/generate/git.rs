use std::path::PathBuf;

use anyhow::Result;

use crate::lib::common::{clone_git_repo, RepoRef};

pub struct CloneTemplate {
    /// temp folder where project will be cloned - deleted after 'wash new' completes
    pub clone_tmp: PathBuf,
    /// github repository URL, e.g., "<https://github.com/wasmcloud/project-templates>".
    /// For convenience, either prefix 'https://' or '<https://github.com>' may be omitted.
    /// ssh urls may be used if ssh-config is setup appropriately.
    /// If a private repository is used, user will be prompted for credentials.
    pub repo_url: String,
    /// sub-folder of project template within the repo, e.g. "component/hello"
    pub sub_folder: Option<String>,
    /// repo branch, e.g., main
    pub repo_branch: String,
}

/// Clone a git template repository
pub async fn clone_git_template(
    CloneTemplate {
        clone_tmp,
        repo_url,
        sub_folder,
        repo_branch,
    }: CloneTemplate,
) -> Result<()> {
    clone_git_repo(
        None,
        &clone_tmp,
        repo_url,
        sub_folder,
        Some(RepoRef::Branch(repo_branch)),
    )
    .await
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
