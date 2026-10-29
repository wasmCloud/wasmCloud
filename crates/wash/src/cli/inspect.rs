use clap::Args;
use tracing::instrument;
use wash_runtime::component_source::ComponentSource;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, oci::RegistryArgs},
    inspect::{decode_component, get_component_wit},
};
use anyhow::{Context, ensure};
use std::path::Path;

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Inspect a component, given either a local path or an OCI reference.
    #[arg(value_name = "COMPONENT_REFERENCE")]
    pub component_reference: String,
    /// Registry settings, used only when the reference is an OCI image.
    #[command(flatten)]
    pub registry: RegistryArgs,
}

impl CliCommand for InspectCommand {
    #[instrument(level = "debug", skip_all, name = "inspect")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let component_reference = &self.component_reference;

        // A directory is the one input the shared source resolution cannot make
        // sense of: it exists, so it is classified as a file, and reading it
        // fails with an OS error that does not say what to do instead.
        let path = Path::new(component_reference);
        ensure!(
            !path.is_dir(),
            "Directory '{component_reference}' specified. Please provide a file path or OCI reference."
        );

        let loaded = ComponentSource::from_reference(component_reference)
            .load(self.registry.oci_config(ctx)?)
            .await?;

        let component = decode_component(loaded.bytes.as_ref())
            .await
            .context("failed to decode component")?;

        // Print the component WIT
        let wit = get_component_wit(component)
            .await
            .context("failed to print component WIT")?;

        Ok(CommandOutput::ok(
            wit.to_owned(),
            Some(serde_json::json!({
                "message": "Component inspected successfully.",
                "success": true,
                "wit": wit,
            })),
        ))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::cli::{OutputKind, test_support::context};
    use anyhow::Result;
    use clap::{Parser, error::ErrorKind};
    use serde_json::json;

    const COMPONENT: &str = r#"(component
        (import "greet" (func (param "name" string) (result string)))
    )"#;
    const COMPONENT_WIT: &str = "package root:component;\n\nworld root {\n  import greet: func(name: string) -> string;\n}\n";
    const PACKAGE_WIT: &str = "package test:inspect@1.2.3;\n\ninterface api {\n  record request {\n    name: string,\n    retries: option<u32>,\n  }\n\n  variant response {\n    ok(string),\n    missing,\n  }\n\n  greet: func(input: request) -> response;\n}\n\nworld demo {\n  import api;\n\n  export run: func() -> result<string, string>;\n}\n";

    #[derive(Parser)]
    struct InspectCli {
        #[command(flatten)]
        command: InspectCommand,
    }

    fn command(path: &Path) -> InspectCommand {
        InspectCommand {
            component_reference: path.to_string_lossy().into_owned(),
            registry: RegistryArgs::default(),
        }
    }

    fn assert_output(output: CommandOutput, expected_wit: &str) -> Result<()> {
        assert!(output.is_success());
        assert_eq!(output.text(), expected_wit);
        let data = json!({
            "message": "Component inspected successfully.",
            "success": true,
            "wit": expected_wit,
        });
        assert_eq!(output.json(), Some(&data));
        assert_eq!(
            output.clone().with_output_kind(OutputKind::Text).render(),
            (expected_wit.into(), true)
        );
        let (rendered, success) = output.with_output_kind(OutputKind::Json).render();
        assert!(success);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered)?,
            json!({
                "message": expected_wit, "success": true, "data": data,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn local_components_print_wit_in_text_and_json_without_changing_the_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let bytes = wat::parse_str(COMPONENT)?;
        for name in [
            "component.wasm",
            "no-extension",
            "component with spaces 🦀.wasm",
        ] {
            let path = temp.path().join(name);
            tokio::fs::write(&path, &bytes).await?;
            assert_output(command(&path).handle(&ctx).await?, COMPONENT_WIT)?;
            assert_eq!(tokio::fs::read(&path).await?, bytes);
        }
        assert!(!ctx.cache_dir().exists());
        assert!(!ctx.data_dir().exists());
        Ok(())
    }

    #[tokio::test]
    async fn encoded_wit_packages_preserve_versions_types_imports_and_exports() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let empty = wat::parse_str("(component)")?;
        let decoded = wit_component::decode(&empty)?;
        let mut resolve = decoded.resolve().clone();
        let package = resolve.push_str("inspect.wit", PACKAGE_WIT)?;
        let bytes = wit_component::encode(&resolve, package)?;
        let path = temp.path().join("package.wasm");
        tokio::fs::write(&path, &bytes).await?;
        assert_output(command(&path).handle(&ctx).await?, PACKAGE_WIT)?;
        assert_eq!(tokio::fs::read(&path).await?, bytes);
        Ok(())
    }

    #[tokio::test]
    async fn directories_get_an_actionable_error_before_registry_configuration() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let path = temp.path().join("directory.wasm");
        tokio::fs::create_dir(&path).await?;
        let mut inspect = command(&path);
        inspect.registry.ca_paths = vec![temp.path().join("missing.pem")];
        let error = inspect
            .handle(&ctx)
            .await
            .err()
            .context("expected directory error")?;
        assert_eq!(
            error.to_string(),
            format!(
                "Directory '{}' specified. Please provide a file path or OCI reference.",
                path.display()
            )
        );
        assert!(!ctx.cache_dir().exists());
        Ok(())
    }

    #[tokio::test]
    async fn missing_local_files_report_the_path_instead_of_pulling_an_image() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let path = temp.path().join("missing.wasm");
        let error = command(&path)
            .handle(&ctx)
            .await
            .err()
            .context("expected missing file error")?;
        assert_eq!(
            error.to_string(),
            format!("failed to read component file {}", path.display())
        );
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::NotFound)
        );
        assert!(!ctx.cache_dir().exists());
        Ok(())
    }

    #[tokio::test]
    async fn malformed_oci_references_keep_the_source_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let reference = "registry.invalid/UPPERCASE/component:bad tag";
        let inspect = InspectCommand {
            component_reference: reference.into(),
            registry: RegistryArgs::default(),
        };
        let error = inspect
            .handle(&ctx)
            .await
            .err()
            .context("expected invalid OCI reference error")?;
        assert_eq!(
            error.to_string(),
            format!("invalid OCI reference: {reference}")
        );
        assert!(!ctx.cache_dir().exists());
        Ok(())
    }

    #[tokio::test]
    async fn empty_interfaces_print_an_empty_world() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let path = temp.path().join("empty.wasm");
        for source in [
            "(component)",
            "(module)",
            "(module (func (export \"run\")))",
        ] {
            tokio::fs::write(&path, wat::parse_str(source)?).await?;
            assert_output(
                command(&path).handle(&ctx).await?,
                "package root:component;\n\nworld root {\n}\n",
            )?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn malformed_files_report_decode_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let path = temp.path().join("invalid.wasm");
        let valid = wat::parse_str(COMPONENT)?;
        let truncated = valid
            .get(..valid.len() - 1)
            .context("fixture is empty")?
            .to_vec();
        for (name, bytes) in [
            ("empty", Vec::new()),
            ("text", b"not WebAssembly".to_vec()),
            ("header", b"\0asm".to_vec()),
            ("truncated", truncated),
        ] {
            tokio::fs::write(&path, &bytes).await?;
            let error = command(&path)
                .handle(&ctx)
                .await
                .err()
                .with_context(|| format!("expected decode error for {name}"))?;
            assert_eq!(error.to_string(), "failed to decode component");
            assert!(
                error
                    .chain()
                    .any(|cause| cause.to_string() == "failed to decode component bytes")
            );
            assert_eq!(tokio::fs::read(&path).await?, bytes);
        }
        assert!(!ctx.cache_dir().exists());
        Ok(())
    }

    #[tokio::test]
    async fn registry_configuration_errors_keep_the_ca_path_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let path = temp.path().join("component.wasm");
        tokio::fs::write(&path, wat::parse_str(COMPONENT)?).await?;
        let mut inspect = command(&path);
        let missing = temp.path().join("missing.pem");
        inspect.registry.ca_paths = vec![missing.clone()];
        let error = inspect
            .handle(&ctx)
            .await
            .err()
            .context("expected CA configuration error")?;
        assert_eq!(
            error.to_string(),
            "failed to load --ca-path CA certificates"
        );
        assert!(format!("{error:#}").contains(&missing.display().to_string()));
        assert!(!ctx.cache_dir().exists());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_follow_the_same_file_and_directory_contract() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let path = temp.path().join("component.wasm");
        tokio::fs::write(&path, wat::parse_str(COMPONENT)?).await?;
        let link = temp.path().join("linked.wasm");
        symlink(&path, &link)?;
        assert_output(command(&link).handle(&ctx).await?, COMPONENT_WIT)?;
        let directory = temp.path().join("linked-directory");
        symlink(temp.path(), &directory)?;
        let error = command(&directory)
            .handle(&ctx)
            .await
            .err()
            .context("expected directory error")?;
        assert!(error.to_string().starts_with("Directory '"));
        assert!(error.to_string().contains(&directory.display().to_string()));
        tokio::fs::remove_file(&path).await?;
        let error = command(&link)
            .handle(&ctx)
            .await
            .err()
            .context("expected dangling symlink error")?;
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::NotFound)
        );
        Ok(())
    }

    #[test]
    fn cli_requires_a_reference_and_accepts_registry_options() -> Result<()> {
        let cli = InspectCli::try_parse_from([
            "inspect",
            "registry.example/team/component:tag",
            "--insecure",
            "--user",
            "user",
            "--password",
            "password",
            "--ca-path",
            "first.pem,second.pem",
        ])?;
        assert_eq!(
            cli.command.component_reference,
            "registry.example/team/component:tag"
        );
        assert!(cli.command.registry.insecure);
        assert_eq!(cli.command.registry.user.as_deref(), Some("user"));
        assert_eq!(cli.command.registry.password.as_deref(), Some("password"));
        assert_eq!(
            cli.command.registry.ca_paths,
            [
                std::path::PathBuf::from("first.pem"),
                std::path::PathBuf::from("second.pem")
            ]
        );
        for (args, kind) in [
            (vec!["inspect"], ErrorKind::MissingRequiredArgument),
            (
                vec!["inspect", "one.wasm", "two.wasm"],
                ErrorKind::UnknownArgument,
            ),
            (
                vec!["inspect", "one.wasm", "--unknown"],
                ErrorKind::UnknownArgument,
            ),
            (
                vec!["inspect", "one.wasm", "--user"],
                ErrorKind::InvalidValue,
            ),
        ] {
            let error = InspectCli::try_parse_from(args)
                .err()
                .context("expected argument error")?;
            assert_eq!(error.kind(), kind);
        }
        Ok(())
    }
}
