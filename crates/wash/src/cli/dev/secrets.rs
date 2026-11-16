//! `dev.secrets`: the component sends a placeholder, `wash dev` the value.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use wash_runtime::host::egress_credential::{
    CredentialError, EgressCredentialSource, EgressSecret, SecretRoute, mint_placeholder,
};
use zeroize::Zeroizing;

use crate::config::DevSecret;

/// A dev secret: the environment variable that carries its placeholder, and
/// the secret the host substitutes for it.
pub(super) struct Minted {
    pub env: String,
    pub secret: EgressSecret,
}

/// The value is read once, when the session starts.
struct Captured(Zeroizing<Vec<u8>>);

impl EgressCredentialSource for Captured {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        Ok(self.0.clone())
    }
}

/// Re-read on every request, so a developer can rotate the value in place.
struct FromFile(PathBuf);

impl EgressCredentialSource for FromFile {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        let value = std::fs::read(&self.0).map_err(|_| CredentialError::Unavailable)?;
        Ok(Zeroizing::new(value.trim_ascii_end().to_vec()))
    }
}

pub(super) fn mint(secrets: &[DevSecret], project_dir: &Path) -> anyhow::Result<Vec<Minted>> {
    let mut minted: Vec<Minted> = Vec::with_capacity(secrets.len());
    for entry in secrets {
        ensure!(!entry.env.is_empty(), "a `dev.secrets` entry needs `env`");
        ensure!(
            minted.iter().all(|m| m.env != entry.env),
            "`dev.secrets` names `{}` twice",
            entry.env
        );
        let secret = secret(entry, project_dir)
            .with_context(|| format!("invalid `dev.secrets` entry for `{}`", entry.env))?;
        minted.push(Minted {
            env: entry.env.clone(),
            secret,
        });
    }
    Ok(minted)
}

/// Give each component the placeholders in place of the values.
pub(super) fn deliver(
    minted: &[Minted],
    environment: &mut HashMap<String, String>,
) -> anyhow::Result<()> {
    for m in minted {
        ensure!(
            !environment.contains_key(&m.env),
            "`{}` is set in both `workload.environment` and `dev.secrets`",
            m.env
        );
        environment.insert(m.env.clone(), m.secret.placeholder().to_string());
    }
    Ok(())
}

fn secret(entry: &DevSecret, project_dir: &Path) -> anyhow::Result<EgressSecret> {
    let source: Arc<dyn EgressCredentialSource> = match (&entry.from_env, &entry.from_file) {
        (Some(var), None) => {
            let value =
                std::env::var(var).with_context(|| format!("`{var}` is not set in this shell"))?;
            Arc::new(Captured(Zeroizing::new(value.into_bytes())))
        }
        (None, Some(path)) => {
            let path = project_dir.join(path);
            ensure!(path.is_file(), "`{}` is not a file", path.display());
            Arc::new(FromFile(path))
        }
        _ => bail!("set exactly one of `from_env` and `from_file`"),
    };
    let placeholder = mint_placeholder(entry.placeholder_prefix.as_deref().unwrap_or(""), 0)?;
    let destinations = entry
        .destinations
        .iter()
        .map(|d| d.parse().with_context(|| format!("`{d}` is not a host")))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut secret = EgressSecret::new(placeholder, source)?
        .for_destinations(destinations)?
        .in_query(&entry.query)
        .for_routes(
            entry
                .routes
                .iter()
                .map(|r| route(r))
                .collect::<anyhow::Result<Vec<_>>>()?,
        );
    if !entry.headers.is_empty() {
        let headers = entry
            .headers
            .iter()
            .map(|h| {
                h.parse()
                    .with_context(|| format!("`{h}` is not a header name"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        secret = secret.in_headers(headers)?;
    }
    Ok(secret)
}

fn route(route: &str) -> anyhow::Result<SecretRoute> {
    match route.split_once(' ') {
        Some((method, path)) => SecretRoute::new(
            Some(
                method
                    .parse()
                    .with_context(|| format!("`{method}` is not a method"))?,
            ),
            path.trim(),
        ),
        None => SecretRoute::new(None, route),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> DevSecret {
        DevSecret {
            env: "API_TOKEN".into(),
            from_env: Some("PATH".into()),
            destinations: vec!["https://api.github.com".into()],
            ..Default::default()
        }
    }

    #[test]
    fn the_component_gets_a_placeholder_not_the_value() {
        let minted = mint(&[entry()], Path::new(".")).unwrap();
        let mut environment = HashMap::new();
        deliver(&minted, &mut environment).unwrap();

        let placeholder = &environment["API_TOKEN"];
        assert_eq!(placeholder, minted[0].secret.placeholder());
        assert_ne!(placeholder, &std::env::var("PATH").unwrap());
    }

    #[test]
    fn a_workload_cannot_also_set_the_variable() {
        let minted = mint(&[entry()], Path::new(".")).unwrap();
        let mut environment = HashMap::from([("API_TOKEN".to_string(), "x".to_string())]);
        assert!(deliver(&minted, &mut environment).is_err());
    }

    #[test]
    fn entries_are_checked_before_the_session_starts() {
        let refused = [
            DevSecret {
                destinations: vec!["*.github.com".into()],
                ..entry()
            },
            DevSecret {
                from_env: None,
                ..entry()
            },
            DevSecret {
                from_file: Some("missing".into()),
                ..entry()
            },
            DevSecret {
                from_env: Some("WASH_DEV_SECRET_THAT_IS_NOT_SET".into()),
                ..entry()
            },
            DevSecret {
                headers: vec!["host".into()],
                ..entry()
            },
            DevSecret {
                routes: vec!["GET v1".into()],
                ..entry()
            },
        ];
        for secret in refused {
            assert!(
                mint(std::slice::from_ref(&secret), Path::new(".")).is_err(),
                "{secret:?}"
            );
        }
        assert!(
            mint(&[entry(), entry()], Path::new(".")).is_err(),
            "duplicate env"
        );
    }

    #[test]
    fn a_file_source_is_read_on_every_request() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("token"), "one\n").unwrap();
        let source = FromFile(dir.path().join("token"));
        assert_eq!(source.current().unwrap().as_slice(), b"one");
        std::fs::write(dir.path().join("token"), "two").unwrap();
        assert_eq!(source.current().unwrap().as_slice(), b"two");
        std::fs::remove_file(dir.path().join("token")).unwrap();
        assert_eq!(source.current(), Err(CredentialError::Unavailable));
    }

    #[test]
    fn prefixes_and_routes_are_honoured() {
        let minted = mint(
            &[DevSecret {
                placeholder_prefix: Some("ghp_".into()),
                routes: vec!["GET /repos".into(), "/search".into()],
                ..entry()
            }],
            Path::new("."),
        )
        .unwrap();
        assert!(minted[0].secret.placeholder().starts_with("ghp_wash_ph_"));
    }
}
