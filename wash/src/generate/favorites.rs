//! Favorites
//! If the user has not specified a template with the --git or --path args,
//! the user is asked to choose a template from a "Favorites" file.
//! A favorites file may be provided with the '--favorites' cli option, otherwise,
//! a compiled-in set of defaults is used.
//! The name of the template from the favorites file can be selected
//! with the '--template-name' option. In silent mode, if no template-name
//! is provided, the first entry of the applicable kind is selected.
//!
//! The favorites file should include settings for at least one actor,
//! interface, and provider. 'name' and 'description' are required,
//! and a location of either 'path' or 'git' must be specified, but not both.
//!
//!
//! ```toml
//! [[actor]]
//! name = "template name - required"
//! description = "something about this template - required"
//! path = "optional path to folder of template files on disk"
//! git = "optional github repository url for templates"
//! subfolder = "optional, subdirectory. Only applicable with git"
//! branch = "optional git branch. Only applicable with git"
//!
//! [[actor]]
//! # settings for another actor template, same fields as above
//!
//! [[interface]]
//! # settings for interface template
//!
//! [[interface]]
//! # settings for another interface template
//!
//! [[provider]]
//! # settings for provider template
//!
//! [[provider]]
//! # settings for another provider template
//!
//! ```
//!
//!
//!
// Future: check a well-known git repo location so favorites effectively
// auto-update even if compiled wash binary doesn't.
//
use crate::generate::{any_msg, ProjectKind};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::{fs, path::PathBuf};

/// Paths to locate project templates
#[derive(Debug, Deserialize)]
pub(crate) struct TemplateSource {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) path: Option<String>,
    pub(crate) git: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) subfolder: Option<String>,
}

/// Contents of favorites file
#[derive(Debug, Deserialize)]
pub(crate) struct Favorites {
    #[serde(flatten)]
    pub(crate) templates: std::collections::HashMap<String, Vec<TemplateSource>>,
}

/// default favorites will be used if one isn't provided on the command line
const DEFAULT_FAVORITES: &str = include_str!("./favorites.toml");

/// try to load favorites from three sources:
/// (1) the parameter
/// (2) hardcoded github url (not yet implemented: post MVP)
/// (3) fallback to compiled-in list
pub(crate) fn load_favorites(path: Option<&PathBuf>) -> Result<Favorites> {
    // load parameter file, if provided
    let data = if let Some(path) = path {
        fs::read_to_string(&path)
            .with_context(|| format!("reading favorites file {}", &path.display()))?
    } else {
        DEFAULT_FAVORITES.to_string()
    };
    let fv = toml::from_str::<Favorites>(&data).with_context(|| "parsing favorites".to_string())?;
    Ok(fv)
}

/// Picks one of the available templates for the project kind.
/// If silent mode, picks the default, or the first entry if there is no default.
/// If interactive, and there is more than one option, displays the choices
/// to let the user pick one
pub(crate) fn pick_favorite(
    fav_file: Option<&PathBuf>,
    kind: &ProjectKind,
    silent: bool,
    fav_name: Option<&String>,
) -> Result<TemplateSource> {
    let mut favorites = load_favorites(fav_file)?;
    let fav = match favorites.templates.remove(&kind.to_string()) {
        Some(mut type_favs) if !type_favs.is_empty() => {
            if let Some(name) = &fav_name {
                type_favs
                    .into_iter()
                    .find(|f| &f.name == *name)
                    .ok_or_else(|| {
                        any_msg(
                            &format!(
                                "no {} template with the name '{}'.",
                                &kind.to_string(),
                                name
                            ),
                            "",
                        )
                    })?
            } else {
                let index = if silent || type_favs.len() == 1 {
                    0
                } else {
                    prompt_for_template(&type_favs, "Select a project template:")?
                };
                type_favs.remove(index)
            }
        }
        _ => {
            return Err(any_msg(
                "templates missing for project type",
                &kind.to_string(),
            ))
        }
    };
    Ok(fav)
}

/// Ask user to select one of the templates
fn prompt_for_template(options: &[TemplateSource], prompt: &str) -> Result<usize> {
    let choices = options
        .iter()
        .map(|s| format!("{}: {}", &s.name, &s.description))
        .collect::<Vec<String>>();

    let entry = crate::generate::project_variables::StringEntry {
        default: None,
        choices: Some(choices),
        regex: None,
    };
    crate::generate::interactive::prompt_for_choice(&entry, prompt)
        .map_err(|e| anyhow!("console IO error: {}", e))
}
