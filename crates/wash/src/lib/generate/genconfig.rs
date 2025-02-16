//! This module defines the data structures and settings
//! for a 'project-generate.toml' file,
//! usually located in the root folder of a project,
//! and a 'values.toml' file, used for automated project creation.
//!
// This file is based on config.rs from cargo-generate
//   source: https://github.com/cargo-generate/cargo-generate
//   version: 0.9.0
//   license: MIT/Apache-2.0
//
use crate::lib::generate::TomlMap;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const CONFIG_FILE_NAME: &str = "project-generate.toml";

/// top-level data structure for a project-generate.toml file
#[derive(Deserialize, Debug, Default, PartialEq)]
pub struct Config {
    pub(crate) template: Option<TemplateConfig>,
    #[serde(default)]
    pub(crate) placeholders: Vec<TomlMap>,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct ConfigValues {
    pub(crate) values: TomlMap,
}

/// template parameters for a project
#[derive(Default, Deserialize, Debug, Eq, PartialEq)]
pub struct TemplateConfig {
    /// list of files or file patterns to omit.
    /// syntax for paths is the same as for `.gitignore` files.
    /// All paths are relative to the project root folder
    /// (which should be the same folder where `project-generate.toml` is located).
    #[serde(default)]
    pub(crate) exclude: Vec<String>,

    /// raw files, or file patterns in `gitignore` format,
    /// that should not be processed by the handlebars template engine.
    /// Any binary files, or files containing non-utf8 characters
    /// should be included in the `raw` list.
    /// Generally, text files are safe to be processed by the template
    /// engine even if they do not contain any template expressions.
    /// If you do encounter a text file that causes errors with the template engine,
    /// and does not need to be processed by the template engine,
    /// adding the files to the `raw` list will avoid the errors.
    #[serde(default)]
    pub(crate) raw: Vec<String>,

    /// A list of files to be renamed. Each item is of the form
    /// `{ from="relative-path-to-project-root", to="new-path-or-name" }`
    /// The `to` field is processed by the template engine and
    /// may contain template expressions. For example, if the project name
    /// is "`ImageProcessor`", this expression `{ from="project.md", to = "{{project-name}}.md }`
    /// will result in renaming the file 'project.md' to 'ImageProcessor.md'.
    #[serde(default)]
    pub(crate) rename: Vec<RenameConfig>,
}

#[derive(Clone, Deserialize, Debug, Eq, PartialEq)]
pub struct RenameConfig {
    pub(crate) from: PathBuf,
    pub(crate) to: String,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct TemplateSlotsTable(pub(crate) TomlMap);

impl Config {
    pub(crate) fn from_path<P>(path: &P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let contents = fs::read_to_string(path).with_context(|| {
            format!(
                "Error reading template configuration `{}`",
                &path.as_ref().display()
            )
        })?;
        let config = toml::from_str::<Self>(&contents).with_context(|| {
            format!(
                "Error parsing template configuration '{}'",
                &path.as_ref().display()
            )
        })?;
        Ok(config)
    }

    /// add a path to the exclude list
    pub(crate) fn exclude(&mut self, path: String) {
        if let Some(ref mut tc) = self.template {
            tc.exclude.push(path);
        } else {
            self.template = Some(TemplateConfig {
                exclude: vec![path],
                ..Default::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::{fs::File, io::Write};
    use tempfile::tempdir;
    use toml::Value;

    fn parse_config(contents: &str) -> Result<Config> {
        toml::from_str::<Config>(contents).map_err(|e| anyhow!("invalid config syntax: {}", e))
    }

    #[test]
    fn test_deserializes_config() {
        let test_dir = tempdir().unwrap();
        let config_path = test_dir.path().join(CONFIG_FILE_NAME);
        let mut file = File::create(&config_path).unwrap();

        file.write_all(
            r#"
            [template]
            exclude = ["ignore.txt"]
            raw = [ "data.bin" ]
            rename = [ { from="README.alt.md", to="README.md" } ]
            [[placeholders]]
            name="value"
            type="string"
            prompt="type something"
        "#
            .as_bytes(),
        )
        .unwrap();

        let config = Config::from_path(&config_path).unwrap();

        assert_eq!(
            config.template,
            Some(TemplateConfig {
                exclude: vec!["ignore.txt".into()],
                raw: vec!["data.bin".into()],
                rename: vec![RenameConfig {
                    from: "README.alt.md".into(),
                    to: "README.md".into()
                }]
            })
        );
        assert_eq!(config.placeholders.len(), 1);
    }

    #[test]
    fn config_deser_placeholders() {
        let result = parse_config(
            r#"
            [[placeholders]]
            name="a"
            type = "bool"
            prompt = "foo"
            default = false
            [[placeholders]]
            name="b"
            type = "string"
            prompt = "bar"
            "#,
        );
        if let Err(e) = &result {
            eprintln!("result error: {e}");
        }
        assert!(result.is_ok(), "Config should have parsed");
        let result = result.unwrap();

        assert_eq!(result.placeholders.len(), 2);

        let pa = result.placeholders.first().unwrap();
        let pb = result.placeholders.get(1).unwrap();

        assert_eq!(pa.len(), 4);
        assert_eq!(pb.len(), 3);

        assert_eq!(pa.get("name"), Some(&Value::String("a".into())));
        assert_eq!(pa.get("type"), Some(&Value::String("bool".to_string())));
        assert_eq!(pa.get("prompt"), Some(&Value::String("foo".to_string())));
        assert_eq!(pa.get("default"), Some(&Value::Boolean(false)));

        assert_eq!(pb.get("name"), Some(&Value::String("b".into())));
        assert_eq!(pb.get("type"), Some(&Value::String("string".to_string())));
        assert_eq!(pb.get("prompt"), Some(&Value::String("bar".to_string())));
    }
}
