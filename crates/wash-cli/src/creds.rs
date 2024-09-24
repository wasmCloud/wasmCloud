//! A temporary module to parse NATS credsfiles and translate
//! their contents into a JWT and Seed value
//!
//! The code in this module is largely copied from `https://github.com/nats-io/nats.rs/blob/main/async-nats/src/auth_utils.rs`
//!
//! This module represents a temporary solution to the fact that a host does not support credsfile authentication

use std::path::Path;

use anyhow::{anyhow, Result};
use regex::Regex;
use tokio::fs::read_to_string;

/// Type alias to represent strings that are JWTs
type Jwt = String;

/// Type alias that represents strings which are NATS nkeys
type Seed = String;

/// Helper function to parse a credsfile from a path and return a tuple
/// with the JWT and Seed values that were in the credsfile
pub async fn parse_credsfile<P>(path: P) -> Result<(Jwt, Seed)>
where
    P: AsRef<Path>,
{
    let contents = read_to_string(path).await?;
    let jwt = parse_decorated_jwt(&contents)?;
    let seed = parse_decorated_nkey(&contents)?;

    Ok((jwt, seed))
}

/// Regex that represents user configuration that decorates an nkey
fn user_config_re() -> Result<Regex> {
    Ok(Regex::new(
        r"\s*(?:(?:[-]{3,}.*[-]{3,}\r?\n)([\w\-.=]+)(?:\r?\n[-]{3,}.*[-]{3,}\r?\n))",
    )?)
}

/// Parses a credentials file and returns its user JWT.
fn parse_decorated_jwt(contents: &str) -> Result<String> {
    let capture = user_config_re()?
        .captures_iter(contents)
        .next()
        .ok_or_else(|| anyhow!("cannot parse user JWT from the credentials file"))?;
    Ok(capture[1].to_string())
}

/// Parses a credentials file and returns its nkey.
fn parse_decorated_nkey(contents: &str) -> Result<String> {
    let capture = user_config_re()?
        .captures_iter(contents)
        .nth(1)
        .ok_or_else(|| anyhow!("cannot parse user seed from the credentials file"))?;
    Ok(capture[1].to_string())
}
