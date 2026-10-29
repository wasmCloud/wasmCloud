use super::*;
use crate::cli::DirectoryStrategy;
use std::sync::Arc;

#[derive(Debug)]
struct TestDirectories(PathBuf);
impl DirectoryStrategy for TestDirectories {
    fn home_dir(&self) -> &Path {
        &self.0
    }
    fn config_dir(&self) -> PathBuf {
        self.0.join("user")
    }
    fn data_dir(&self) -> PathBuf {
        self.0.join("data")
    }
    fn cache_dir(&self) -> PathBuf {
        self.0.join("cache")
    }
    fn state_dir(&self) -> Option<PathBuf> {
        None
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        None
    }
}
fn context(root: &Path) -> CliContext {
    CliContext {
        app_strategy: Arc::new(TestDirectories(root.into())),
        non_interactive: true,
        config: None,
        project_dir: root.into(),
        original_working_dir: root.into(),
        meters: Default::default(),
    }
}

#[tokio::test]
async fn init_formats_preserve_existing_files_unless_forced() -> anyhow::Result<()> {
    for format in ["yaml", "yml", "json", "toml"] {
        for example in [false, true] {
            let temp = tempfile::tempdir()?;
            let ctx = context(temp.path());
            let mut arguments = vec!["config", "init", "--format", format];
            if example {
                arguments.push("--example");
            }
            let args = ConfigArgs::try_parse_from(&arguments)?;
            let result = args.handle(&ctx).await?;
            assert!(result.is_success());
            let path = ctx.project_dir().join(".wash").join(format!(
                "config.{}",
                if format == "yml" { "yaml" } else { format }
            ));
            let contents = tokio::fs::read(&path).await?;
            assert!(!contents.is_empty());
            assert!(load_config_from_file(&path).is_ok());
            assert!(args.handle(&ctx).await.is_err());
            assert_eq!(tokio::fs::read(&path).await?, contents);
            arguments.extend(["--force", "--global"]);
            assert!(
                ConfigArgs::try_parse_from(arguments)?
                    .handle(&ctx)
                    .await?
                    .is_success()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn cleanup_reports_sizes_and_respects_dry_run_and_selected_directories() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let ctx = context(temp.path());
    assert!(cmd_cleanup(&ctx, false, false, false, false).await.is_err());
    tokio::fs::create_dir_all(ctx.cache_dir().join("nested")).await?;
    tokio::fs::create_dir_all(ctx.data_dir()).await?;
    tokio::fs::write(ctx.cache_dir().join("nested/file"), b"abc").await?;
    tokio::fs::write(ctx.data_dir().join("file"), b"12345").await?;
    let dry = cmd_cleanup(&ctx, false, false, true, true).await?;
    assert_eq!(
        dry.json().and_then(|j| j.get("total_bytes")),
        Some(&serde_json::json!(8))
    );
    assert!(ctx.cache_dir().exists() && ctx.data_dir().exists());
    assert!(dry.text().contains("would remove"));
    cmd_cleanup(&ctx, true, false, false, false).await?;
    assert!(!ctx.cache_dir().exists());
    assert!(ctx.data_dir().exists());
    let missing = cmd_cleanup(&ctx, true, false, false, true).await?;
    assert!(missing.text().contains("would skip (missing)"));
    let removed = cmd_cleanup(&ctx, false, false, true, false).await?;
    assert!(removed.text().contains("skipped (missing)"));
    assert!(!ctx.data_dir().exists());
    assert!(dir_size(&ctx.data_dir()).await.is_err());
    Ok(())
}

#[tokio::test]
async fn validation_and_info_expose_structured_results() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let ctx = context(temp.path());
    let info = ConfigCommand::Info.handle(&ctx).await?;
    assert_eq!(
        info.json().and_then(|j| j.get("version")),
        Some(&serde_json::json!(env!("CARGO_PKG_VERSION")))
    );
    assert!(ConfigCommand::Show.handle(&ctx).await?.is_success());
    let default = ConfigCommand::Validate { file: None }.handle(&ctx).await?;
    assert!(default.is_success());
    assert!(default.text().contains("built-in defaults"));
    let file = temp.path().join("explicit.yaml");
    assert!(cmd_validate(&ctx, Some(&file)).await.is_err());
    tokio::fs::write(&file, "[").await?;
    assert!(cmd_validate(&ctx, Some(&file)).await.is_err());
    tokio::fs::write(&file, "{}").await?;
    assert!(cmd_validate(&ctx, Some(&file)).await?.is_success());
    tokio::fs::write(&file, "build:\n  command: ''\n  component_path: ''\n").await?;
    let invalid = cmd_validate(&ctx, Some(&file)).await?;
    assert!(!invalid.is_success());
    assert_eq!(
        invalid.json().and_then(|j| j.get("valid")),
        Some(&serde_json::json!(false))
    );
    Ok(())
}
