//! Helpful utilities for retrieving user input from the command line
// This file is lightly modified from interactive.rs from cargo-generate
//   source: https://github.com/cargo-generate/cargo-generate
//   version: 0.9.0
//   license: MIT/Apache-2.0
//
use crate::lib::generate::{
    any_msg,
    project_variables::{StringEntry, TemplateSlots, VarInfo},
    PROJECT_NAME_REGEX,
};
use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Input};
use serde_json::Value;
use std::ops::Index;

pub(crate) fn name() -> Result<String> {
    let valid_ident = regex::Regex::new(PROJECT_NAME_REGEX).unwrap();
    //r"^([a-zA-Z][a-zA-Z0-9_-]+)$"
    let project_var = TemplateSlots {
        var_name: "crate_name".into(),
        prompt: "Project Name".into(),
        var_info: VarInfo::String {
            entry: StringEntry {
                default: None,
                choices: None,
                regex: Some(valid_ident),
            },
        },
    };
    prompt_for_variable(&project_var)
}

pub fn user_question(prompt: &str, default: &Option<String>) -> Result<String> {
    let mut i = Input::<String>::new().with_prompt(prompt.to_string());
    if let Some(s) = default {
        i = i.default(s.to_owned());
    }
    i.interact().map_err(anyhow::Error::from)
}

fn extract_default(variable: &VarInfo) -> Option<String> {
    match variable {
        VarInfo::Bool {
            default: Some(d), ..
        } => Some(if *d { "true".into() } else { "false".into() }),
        VarInfo::String {
            entry: StringEntry {
                default: Some(d), ..
            },
        } => Some(d.into()),
        _ => None,
    }
}

pub fn prompt_for_choice(entry: &StringEntry, prompt: &str) -> Result<usize> {
    use dialoguer::Select;
    let choices = entry.choices.as_ref().unwrap();

    let default = if let Some(default) = &entry.default {
        choices.binary_search(default).unwrap_or(0)
    } else {
        0
    };
    let chosen = Select::with_theme(&ColorfulTheme::default())
        .items(choices)
        .with_prompt(prompt)
        .default(default)
        .interact()?;
    Ok(chosen)
}

pub(crate) fn prompt_for_variable(variable: &TemplateSlots) -> Result<String> {
    let prompt = format!(
        "{} {}",
        crate::lib::generate::emoji::SHRUG,
        style(&variable.prompt).bold(),
    );

    if let VarInfo::String { entry } = &variable.var_info {
        if let Some(choices) = &entry.choices {
            let chosen = prompt_for_choice(entry, &prompt)?;
            return Ok(choices.index(chosen).to_string());
        }
    }

    let prompt = format!("{} {}", prompt, choice_options(&variable.var_info));
    loop {
        let default = extract_default(&variable.var_info);
        let user_entry = user_question(prompt.as_str(), &default)?;

        if is_valid_variable_value(&user_entry, &variable.var_info) {
            return Ok(user_entry);
        } else {
            eprintln!(
                "{} {} \"{}\" {}",
                crate::lib::generate::emoji::WARN,
                style("Sorry,").bold().red(),
                style(&user_entry).bold().yellow(),
                style(format!("is not a valid value for {}", variable.var_name))
                    .bold()
                    .red()
            );
        }
    }
}

pub(super) fn variable(variable: &TemplateSlots) -> Result<Value> {
    let user_input = prompt_for_variable(variable)?;
    into_value(user_input, &variable.var_info)
}

fn is_valid_variable_value(user_entry: &str, var_info: &VarInfo) -> bool {
    match var_info {
        VarInfo::Bool { .. } => user_entry.parse::<bool>().is_ok(),
        VarInfo::String { entry } => match entry {
            StringEntry {
                choices: Some(options),
                regex: Some(reg),
                ..
            } => options.iter().any(|x| x == user_entry) && reg.is_match(user_entry),
            StringEntry {
                choices: Some(options),
                regex: None,
                ..
            } => options.iter().any(|x| x == user_entry),
            StringEntry {
                choices: None,
                regex: Some(reg),
                ..
            } => reg.is_match(user_entry),
            StringEntry {
                choices: None,
                regex: None,
                ..
            } => true,
        },
    }
}

fn into_value(user_entry: String, var_info: &VarInfo) -> Result<Value> {
    match var_info {
        VarInfo::Bool { .. } => {
            let as_bool = user_entry
                .parse::<bool>()
                .map_err(|e| any_msg("Invalid boolean value for template", &e.to_string()))?; // this shouldn't fail if checked before
            Ok(Value::Bool(as_bool))
        }
        VarInfo::String { .. } => Ok(Value::String(user_entry)),
    }
}

fn choice_options(var_info: &VarInfo) -> String {
    match var_info {
        VarInfo::Bool { default: None } => "[true, false]".to_string(),
        VarInfo::Bool { default: Some(d) } => {
            format!("[true, false] [default: {}]", style(d).bold())
        }
        VarInfo::String { entry } => match entry {
            StringEntry {
                choices: Some(ref cs),
                default: None,
                ..
            } => format!("[{}]", cs.join(", ")),
            StringEntry {
                choices: Some(ref cs),
                default: Some(ref d),
                ..
            } => {
                format!("[{}] [default: {}]", cs.join(", "), style(d).bold())
            }
            StringEntry {
                choices: None,
                default: Some(ref d),
                ..
            } => {
                format!("[default: {}]", style(d).bold())
            }
            _ => String::new(),
        },
    }
}
