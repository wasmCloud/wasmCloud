//! template.rs
//! Process files in template folder to build project folder
//!
use crate::lib::generate::{
    any_msg,
    genconfig::{RenameConfig, TemplateConfig},
    ParamMap,
};
use anyhow::{bail, Context, Result};
use console::style;
use handlebars::Handlebars;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use path_absolutize::Absolutize;
use std::{
    convert::AsRef,
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// Matcher determines disposition of file: whether it should be copied, whether translated with template engine, and whether it is renamed
/// The exclude and raw lists use `GitIgnore` pattern matching
struct Matcher {
    exclude: Option<Gitignore>,
    raw: Option<Gitignore>,
    rename: Vec<RenameConfig>,
}

impl Matcher {
    fn new<P: AsRef<Path>>(project_dir: P, template_config: &TemplateConfig) -> Result<Self> {
        let exclude = if !template_config.exclude.is_empty() {
            Some(create_matcher(
                project_dir.as_ref(),
                &template_config.exclude,
            )?)
        } else {
            None
        };
        let raw = if !template_config.raw.is_empty() {
            Some(create_matcher(project_dir.as_ref(), &template_config.raw)?)
        } else {
            None
        };
        let rename = template_config.rename.clone();
        Ok(Self {
            exclude,
            raw,
            rename,
        })
    }

    /// determine whether a file should be included, based on exclude patterns
    fn should_include(&self, rel_path: &Path) -> bool {
        if let Some(exclude) = &self.exclude {
            !exclude
                .matched_path_or_any_parents(rel_path, /* is_dir */ false)
                .is_ignore()
        } else {
            true
        }
    }

    /// determine renamed destination path
    fn rename_path(&self, rel_path: &Path) -> Option<&str> {
        let ren = self
            .rename
            .iter()
            .find(|rc| rc.from == rel_path)
            .map(|rc| rc.to.as_str());
        ren
    }

    /// determine whether the file should be copied directly, or processed with the template engine
    fn is_raw(&self, rel_path: &Path) -> bool {
        if let Some(raw) = &self.raw {
            raw.matched_path_or_any_parents(rel_path, /* is_dir */ false)
                .is_ignore()
        } else {
            false
        }
    }
}

fn create_matcher<P: AsRef<Path>>(project_dir: P, patterns: &[String]) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(project_dir);
    for rule in patterns {
        builder.add_line(None, rule)?;
    }
    Ok(builder.build()?)
}

pub fn spinner() -> Result<ProgressStyle> {
    Ok(ProgressStyle::default_spinner()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
        .template("{prefix:.bold.dim} {spinner} {wide_msg}")?)
}

pub fn process_template_dir(
    source_dir: &Path,
    project_dir: &Path,
    template_config: &TemplateConfig,
    renderer: &Handlebars,
    values: &ParamMap,
    mp: &mut MultiProgress,
) -> Result<()> {
    fn is_git_metadata(entry: &Path) -> bool {
        entry
            .components()
            .any(|c| c == std::path::Component::Normal(".git".as_ref()))
    }

    let matcher = Matcher::new(source_dir, template_config)?;
    let spinner_style = spinner()?;

    let files = WalkDir::new(source_dir)
        .sort_by_file_name() // ensure deterministic order and easier-to-read progress output
        .contents_first(true) // contents before their directory
        .follow_links(false) // do not follow symlinks
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| !is_git_metadata(e.path()))
        .filter(|e| e.path() != source_dir)
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<PathBuf>>();

    let total = files.len().to_string();
    for (progress, entry) in files.into_iter().enumerate() {
        let pb = mp.add(ProgressBar::new(50));
        pb.set_style(spinner_style.clone());
        pb.set_prefix(format!(
            "[{:width$}/{}]",
            progress + 1,
            total,
            width = total.len()
        ));

        let filename = entry.as_path();
        let src_relative = if filename.starts_with(source_dir) {
            filename.strip_prefix(source_dir)?
        } else {
            filename
        };
        let f = src_relative.display();
        pb.set_message(format!("Processing: {f}"));

        if matcher.should_include(src_relative) {
            if entry.is_file() {
                let dest_rel_path = if let Some(rename_path) = matcher.rename_path(src_relative) {
                    // allow file paths to contain templates using previously defined variables
                    PathBuf::from(renderer.render_template(rename_path, values).with_context(
                        || {
                            format!(
                                "error processing template filename '{}'. Project variables: {:?}",
                                rename_path, &values
                            )
                        },
                    )?)
                } else {
                    src_relative.to_path_buf()
                };
                let dest_path = project_dir.join(&dest_rel_path);
                // convert to absolute canonical path for safety check
                let dest_path = dest_path.absolutize().with_context(|| {
                    format!(
                        "invalid file destination path: {}",
                        &dest_rel_path.display()
                    )
                })?;
                // Safety check: block attempts to write outside project dir
                if !dest_path.starts_with(project_dir) {
                    bail!(
                        "invalid destination: {} is not within project dir",
                        &dest_path.display()
                    );
                }
                if dest_path.exists() {
                    bail!(
                        "Destination file '{}' exists: quitting!",
                        &dest_path.display()
                    );
                }
                fs::create_dir_all(dest_path.parent().unwrap()).unwrap();
                if matcher.is_raw(src_relative) {
                    fs::copy(&entry, &dest_path)?;
                } else {
                    let contents = fs::read_to_string(&entry).with_context(|| {
                            format!(
                                "{} {} `{}` {}",
                                crate::lib::generate::emoji::ERROR,
                                style("Error reading template file.").bold().red(),
                                style(&entry.display()).bold(),
                                "If this is not a text file, you may want to add the path to the 'template.raw' list in project-generate.toml"
                            )
                        })?;
                    let rendered = renderer.render_template(&contents, values).map_err(|e| {
                        any_msg(
                            &format!("rendering template file {}", &src_relative.display()),
                            &e.to_string(),
                        )
                    })?;
                    fs::write(&dest_path, rendered.as_bytes()).with_context(|| {
                        format!(
                            "{} {} `{}`",
                            crate::lib::generate::emoji::ERROR,
                            style("Error saving rendered file:").bold().red(),
                            style(dest_path.display()).bold()
                        )
                    })?;
                    let f = &dest_rel_path.display();
                    pb.inc(50);
                    pb.finish_with_message(format!("Done: {f}"));
                }
            } // not file
        } else {
            pb.finish_with_message(format!("Skipped: {f}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn match_rename() {
        let template_config = TemplateConfig {
            exclude: Vec::new(),
            raw: Vec::new(),
            rename: vec![RenameConfig {
                from: "a.txt".into(),
                to: "b.txt".into(),
            }],
        };

        let matcher = Matcher::new("/target", &template_config).unwrap();

        assert_eq!(matcher.rename_path(&PathBuf::from("README.txt")), None);
        assert_eq!(matcher.rename_path(&PathBuf::from("a.txt")), Some("b.txt"));
    }

    #[test]
    fn match_exclude() {
        let template_config = TemplateConfig {
            exclude: vec!["*.txt".into(), ".gitignore".into()],
            raw: Vec::new(),
            rename: Vec::new(),
        };

        let matcher = Matcher::new("/target", &template_config).unwrap();
        assert!(!matcher.should_include(&PathBuf::from("a.txt")));
        assert!(matcher.should_include(&PathBuf::from("a.txt.html")));
    }

    #[test]
    fn match_raw() {
        let template_config = TemplateConfig {
            exclude: Vec::new(),
            raw: vec!["*.bin".into(), "b.dat".into()],
            rename: vec![RenameConfig {
                from: "a.bin".into(),
                to: "b.bin".into(),
            }],
        };

        let matcher = Matcher::new("/target", &template_config).unwrap();

        assert!(matcher.is_raw(&PathBuf::from("a.bin")));
        assert!(matcher.is_raw(&PathBuf::from("x.bin")));
        assert!(matcher.is_raw(&PathBuf::from("b.dat")));
    }
}
