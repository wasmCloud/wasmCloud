//! Functions for creating new projects from git repositories

use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::process::Command;
use tracing::{debug, error, info, instrument};

/// Extract a specific subfolder from the cloned template
#[instrument(level = "debug", skip_all)]
pub(crate) async fn extract_subfolder(
    source_dir: &Path,
    output_dir: &Path,
    subfolder: &str,
) -> anyhow::Result<()> {
    let subfolder_path = source_dir.join(subfolder);

    if tokio::fs::metadata(&subfolder_path).await.is_err() {
        bail!("Subfolder '{subfolder}' does not exist in cloned repository");
    }

    let metadata = tokio::fs::metadata(&subfolder_path)
        .await
        .context("failed to read subfolder metadata")?;

    if !metadata.is_dir() {
        bail!("subfolder '{subfolder}' is not a directory");
    }

    info!(subfolder = %subfolder, "extracting subfolder");

    // Move subfolder contents
    tokio::fs::create_dir_all(&output_dir)
        .await
        .context("failed to create output directory")?;
    copy_dir_recursive(&subfolder_path, output_dir).await?;

    info!(subfolder, "successfully extracted subfolder",);
    Ok(())
}

/// Clone a repository from a git URL
///
/// NOTE: This requires the `git` command to be available in the system PATH.
#[instrument(level = "debug", skip_all)]
pub(crate) async fn clone_template(
    url: &str,
    output_dir: &Path,
    git_ref: Option<&str>,
) -> anyhow::Result<()> {
    debug!(url, output_dir = %output_dir.display(), git_ref, "cloning repository");

    info!(url, "cloning git repository");
    let output_dir_str = output_dir.to_string_lossy();
    let mut clone_args = vec!["clone"];
    // When no ref is requested, a shallow clone is enough and much faster.
    // When a ref is requested it may be a commit SHA that isn't at the tip
    // of the default branch, so the full history is fetched and resolved by
    // the checkout below.
    if git_ref.is_none() {
        clone_args.extend(["--depth", "1"]);
    }
    clone_args.extend([url, &output_dir_str]);
    run_git(clone_args, None, "git clone failed").await?;

    if let Some(git_ref) = git_ref {
        info!("Using git reference: {}", git_ref);
        run_git(
            ["checkout", git_ref],
            Some(output_dir),
            "git checkout failed",
        )
        .await?;
    }

    info!(output_dir = %output_dir.display(), "Successfully cloned repository");

    // Remove .git directory to avoid confusion
    let git_dir = output_dir.join(".git");
    if git_dir.exists() {
        debug!("Removing .git directory from cloned repository");
        tokio::fs::remove_dir_all(&git_dir)
            .await
            .context("Failed to remove .git directory")?;
    }

    Ok(())
}

async fn run_git<I, S>(
    args: I,
    current_dir: Option<&Path>,
    failure_message: &str,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.args(args);
    // Prevent git from prompting for credentials, which would hang in non-interactive environments.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(current_dir) = current_dir {
        cmd.current_dir(current_dir);
    }

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute git command: {failure_message}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(stderr = %stderr, failure_message);
        bail!("{failure_message}: {stderr}");
    }
}

/// Recursively copy a directory using tokio::fs. Note that the boxing is necessary to allow for async recursion.
pub(crate) fn copy_dir_recursive<'a>(
    src: impl AsRef<std::path::Path> + Send + 'a,
    dst: impl AsRef<std::path::Path> + Send + 'a,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let src = src.as_ref();
        let dst = dst.as_ref();

        let src_metadata = tokio::fs::metadata(src)
            .await
            .with_context(|| format!("Failed to read source path: {src}", src = src.display()))?;

        if !src_metadata.is_dir() {
            bail!("Source is not a directory: {src}", src = src.display());
        }

        tokio::fs::create_dir_all(dst)
            .await
            .with_context(|| format!("Failed to create directory: {dst}", dst = dst.display()))?;

        let mut entries = tokio::fs::read_dir(src)
            .await
            .with_context(|| format!("Failed to read directory: {src}", src = src.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .context("Failed to read directory entry")?
        {
            let path = entry.path();
            let name = entry.file_name();
            let dst_path = dst.join(&name);

            // Skip .git directories
            if name == ".git" {
                debug!("Skipping .git directory");
                continue;
            }

            let metadata = entry
                .metadata()
                .await
                .context("Failed to read entry metadata")?;

            if metadata.is_dir() {
                copy_dir_recursive(&path, &dst_path).await?;
            } else {
                tokio::fs::copy(&path, &dst_path).await.with_context(|| {
                    format!(
                        "Failed to copy file {} to {}",
                        path.display(),
                        dst_path.display()
                    )
                })?;
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::clone_template;
    use tokio::process::Command;

    async fn run_git_in_repo(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .await
            .expect("git command should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn current_commit(repo: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .await
                .expect("rev-parse should run")
                .stdout,
        )
        .expect("commit should be utf-8")
        .trim()
        .to_string()
    }

    async fn init_test_repo() -> (tempfile::TempDir, PathBuf, String, String) {
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let repo = tempdir.path().join("template");
        tokio::fs::create_dir(&repo)
            .await
            .expect("repo directory should be created");

        run_git_in_repo(&repo, &["init", "-b", "main"]).await;
        run_git_in_repo(&repo, &["config", "user.name", "test"]).await;
        run_git_in_repo(&repo, &["config", "user.email", "test@example.com"]).await;

        // Prevent git from rewriting line endings on Windows where
        // `core.autocrlf=true` is the default, so cloned file contents
        // match the bytes we committed.
        tokio::fs::write(repo.join(".gitattributes"), "* -text\n")
            .await
            .expect(".gitattributes should be written");
        run_git_in_repo(&repo, &["add", ".gitattributes"]).await;
        run_git_in_repo(&repo, &["commit", "-m", "gitattributes"]).await;

        let readme = repo.join("README.md");
        tokio::fs::write(&readme, "first\n")
            .await
            .expect("first revision should be written");
        run_git_in_repo(&repo, &["add", "README.md"]).await;
        run_git_in_repo(&repo, &["commit", "-m", "first"]).await;

        let first_commit = current_commit(&repo).await;
        run_git_in_repo(&repo, &["tag", "v1.0.0"]).await;

        tokio::fs::write(&readme, "second\n")
            .await
            .expect("second revision should be written");
        run_git_in_repo(&repo, &["commit", "-am", "second"]).await;

        let second_commit = current_commit(&repo).await;
        run_git_in_repo(&repo, &["branch", "feature/test-ref"]).await;

        (tempdir, repo, first_commit, second_commit)
    }

    #[tokio::test]
    async fn clone_template_supports_commit_refs() {
        let (_tempdir, repo, first_commit, second_commit) = init_test_repo().await;
        let clone_dir = repo
            .parent()
            .expect("repo should have parent")
            .join("clone-by-commit");

        clone_template(&repo.to_string_lossy(), &clone_dir, Some(&first_commit))
            .await
            .expect("cloning by commit SHA should succeed");

        let readme = tokio::fs::read_to_string(clone_dir.join("README.md"))
            .await
            .expect("cloned README should exist");
        assert_eq!(readme, "first\n");
        assert_ne!(first_commit, second_commit);
        assert!(!clone_dir.join(".git").exists());
    }

    #[tokio::test]
    async fn clone_template_supports_branch_refs() {
        let (_tempdir, repo, _first_commit, _second_commit) = init_test_repo().await;
        let clone_dir = repo
            .parent()
            .expect("repo should have parent")
            .join("clone-by-branch");

        clone_template(
            &repo.to_string_lossy(),
            &clone_dir,
            Some("feature/test-ref"),
        )
        .await
        .expect("cloning by branch should succeed");

        let readme = tokio::fs::read_to_string(clone_dir.join("README.md"))
            .await
            .expect("cloned README should exist");
        assert_eq!(readme, "second\n");
        assert!(!clone_dir.join(".git").exists());
    }

    #[tokio::test]
    async fn clone_template_supports_tag_refs() {
        let (_tempdir, repo, first_commit, _second_commit) = init_test_repo().await;
        let clone_dir = repo
            .parent()
            .expect("repo should have parent")
            .join("clone-by-tag");

        clone_template(&repo.to_string_lossy(), &clone_dir, Some("v1.0.0"))
            .await
            .expect("cloning by tag should succeed");

        let readme = tokio::fs::read_to_string(clone_dir.join("README.md"))
            .await
            .expect("cloned README should exist");
        assert_eq!(readme, "first\n");
        assert_eq!(first_commit.len(), 40);
        assert!(!clone_dir.join(".git").exists());
    }

    #[tokio::test]
    async fn clone_template_without_ref_clones_default_branch_tip() {
        let (_tempdir, repo, _first_commit, _second_commit) = init_test_repo().await;
        let clone_dir = repo
            .parent()
            .expect("repo should have parent")
            .join("clone-no-ref");

        clone_template(&repo.to_string_lossy(), &clone_dir, None)
            .await
            .expect("cloning without a ref should succeed");

        let readme = tokio::fs::read_to_string(clone_dir.join("README.md"))
            .await
            .expect("cloned README should exist");
        assert_eq!(readme, "second\n");
        assert!(!clone_dir.join(".git").exists());
    }
}
