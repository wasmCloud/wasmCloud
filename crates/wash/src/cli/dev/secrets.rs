//! `dev.secrets`: the component sends a placeholder, `wash dev` the value.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use tracing::warn;
use wash_runtime::host::egress_credential::{
    CredentialError, EgressCredentialSource, EgressSecret, MIN_SECRET_LEN, SecretForm, SecretRoute,
    mint_placeholder,
};
use zeroize::Zeroizing;

use crate::config::DevSecret;

/// How often a `fromFile` value is read again, off the request path.
const FILE_REFRESH: Duration = Duration::from_secs(1);

/// The largest `fromFile` value read.
const MAX_FILE_LEN: u64 = 64 * 1024;

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

fn captured(value: Zeroizing<Vec<u8>>) -> anyhow::Result<Captured> {
    ensure!(
        value.len() >= MIN_SECRET_LEN,
        "the value is shorter than {MIN_SECRET_LEN} bytes"
    );
    Ok(Captured(value))
}

type Latest = RwLock<Result<Zeroizing<Vec<u8>>, CredentialError>>;

/// Read when the session starts, then refreshed in the background, so a
/// developer can rotate the value in place without a request ever waiting on
/// the file.
struct FromFile(Arc<Latest>);

impl FromFile {
    fn watch(path: PathBuf, every: Duration) -> anyhow::Result<Self> {
        let latest = Arc::new(RwLock::new(Ok(read_value(&path)?)));
        let watched = Arc::downgrade(&latest);
        std::thread::Builder::new()
            .name("wash-dev-secret".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(every);
                    let Some(latest) = watched.upgrade() else {
                        return;
                    };
                    let next = read_value(&path);
                    let mut current = latest.write().unwrap_or_else(|e| e.into_inner());
                    if let (Ok(_), Err(err)) = (&*current, &next) {
                        warn!(
                            path = %path.display(),
                            err = %err,
                            "dev secret file is unreadable; requests carrying it are refused"
                        );
                    }
                    *current = next.map_err(|_| CredentialError::Unavailable);
                }
            })
            .context("could not start watching the file")?;
        Ok(Self(latest))
    }
}

impl EgressCredentialSource for FromFile {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The file's value, without trailing whitespace. Errors never show it.
fn read_value(path: &Path) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("could not read `{}`", path.display()))?;
    let mut value = Zeroizing::new(Vec::new());
    file.take(MAX_FILE_LEN + 1)
        .read_to_end(&mut value)
        .with_context(|| format!("could not read `{}`", path.display()))?;
    ensure!(
        u64::try_from(value.len()).is_ok_and(|len| len <= MAX_FILE_LEN),
        "`{}` is larger than {MAX_FILE_LEN} bytes",
        path.display()
    );
    let value = Zeroizing::new(value.trim_ascii_end().to_vec());
    ensure!(
        value.len() >= MIN_SECRET_LEN,
        "`{}` holds a value shorter than {MIN_SECRET_LEN} bytes",
        path.display()
    );
    Ok(value)
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

/// Refuse a component whose own environment replaces a placeholder it would
/// otherwise inherit from the workload.
pub(super) fn ensure_inherited(
    secrets: &[DevSecret],
    workload: &HashMap<String, String>,
    component: &HashMap<String, String>,
    name: &str,
) -> anyhow::Result<()> {
    for secret in secrets {
        ensure!(
            component.get(&secret.env) == workload.get(&secret.env),
            "`{}` is set in both `dev.components['{name}'].environment` and `dev.secrets`",
            secret.env
        );
    }
    Ok(())
}

fn secret(entry: &DevSecret, project_dir: &Path) -> anyhow::Result<EgressSecret> {
    let source: Arc<dyn EgressCredentialSource> = match (&entry.from_env, &entry.from_file) {
        (Some(var), None) => {
            let value = std::env::var_os(var)
                .with_context(|| format!("`{var}` is not set in this shell"))?;
            Arc::new(captured(Zeroizing::new(value.into_encoded_bytes()))?)
        }
        (None, Some(path)) => {
            let path = project_dir.join(path);
            ensure!(path.is_file(), "`{}` is not a file", path.display());
            if let Ok(canonical) = path.canonicalize() {
                wash_runtime::config_source::warn_if_in_repo(
                    &canonical,
                    Some(project_dir),
                    &entry.env,
                );
            }
            Arc::new(FromFile::watch(path, FILE_REFRESH)?)
        }
        _ => bail!("set exactly one of `fromEnv` and `fromFile`"),
    };
    let placeholder = mint_placeholder(entry.placeholder_prefix.as_deref().unwrap_or(""), 0)?;
    let destinations = entry
        .destinations
        .iter()
        .map(|d| {
            d.parse()
                .with_context(|| format!("`{d}` is not an https host"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let secret =
        EgressSecret::new(placeholder, source, form(entry)?)?.for_destinations(destinations)?;
    if entry.routes.is_empty() {
        return Ok(secret.any_route());
    }
    secret.for_routes(
        entry
            .routes
            .iter()
            .map(|r| route(r))
            .collect::<anyhow::Result<Vec<_>>>()?,
    )
}

/// The one place the placeholder goes: a query parameter, a header's whole
/// value, or by default a bearer token.
fn form(entry: &DevSecret) -> anyhow::Result<SecretForm> {
    match (&entry.header, &entry.query) {
        (Some(_), Some(_)) => bail!("a secret goes in one header or one query parameter"),
        (None, Some(name)) => Ok(SecretForm::Query(name.clone())),
        (None, None) => Ok(SecretForm::Bearer),
        (Some(name), None) if name.eq_ignore_ascii_case("authorization") => Ok(SecretForm::Bearer),
        (Some(name), None) => {
            Ok(SecretForm::Header(name.parse().with_context(|| {
                format!("`{name}` is not a header name")
            })?))
        }
    }
}

/// The standard methods a route may name, in any case.
const METHODS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "TRACE", "CONNECT",
];

/// `/path` or `METHOD /path`.
fn route(route: &str) -> anyhow::Result<SecretRoute> {
    let mut words = route.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(path), None, None) => SecretRoute::new(None, path),
        (Some(method), Some(path), None) => {
            let method = METHODS
                .iter()
                .find(|m| m.eq_ignore_ascii_case(method))
                .with_context(|| format!("`{method}` is not a standard HTTP method"))?;
            SecretRoute::new(Some(method.parse()?), path)
        }
        _ => bail!("`{route}` is not `/path` or `METHOD /path`"),
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
    fn a_component_cannot_replace_the_placeholder() {
        let minted = mint(&[entry()], Path::new(".")).unwrap();
        let mut workload = HashMap::new();
        deliver(&minted, &mut workload).unwrap();

        let inherited = workload.clone();
        assert!(ensure_inherited(&[entry()], &workload, &inherited, "sidecar").is_ok());
        let mut replaced = workload.clone();
        replaced.insert("API_TOKEN".into(), "x".into());
        let err = ensure_inherited(&[entry()], &workload, &replaced, "sidecar").unwrap_err();
        assert!(err.to_string().contains("sidecar"), "{err}");
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
                header: Some("host".into()),
                ..entry()
            },
            DevSecret {
                header: Some("x-api-key".into()),
                query: Some("key".into()),
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
    fn routes_are_a_path_or_a_standard_method_and_path() {
        for refused in [
            "GET v1",
            "FETCH /v1",
            "GET /v1 extra",
            "GET",
            "",
            "/v1/../x",
        ] {
            assert!(route(refused).is_err(), "{refused:?}");
        }
        for allowed in ["/v1", "GET /v1", "get /v1", "  DELETE   /v1/x  "] {
            assert!(route(allowed).is_ok(), "{allowed:?}");
        }
    }

    #[test]
    fn the_form_follows_the_header_or_query_named() {
        let form_of = |header: Option<&str>, query: Option<&str>| {
            form(&DevSecret {
                header: header.map(Into::into),
                query: query.map(Into::into),
                ..entry()
            })
            .unwrap()
        };
        assert_eq!(form_of(None, None), SecretForm::Bearer);
        assert_eq!(form_of(Some("Authorization"), None), SecretForm::Bearer);
        assert_eq!(
            form_of(Some("x-api-key"), None),
            SecretForm::Header("x-api-key".parse().unwrap())
        );
        assert_eq!(form_of(None, Some("key")), SecretForm::Query("key".into()));
    }

    #[test]
    fn a_short_value_is_refused_when_the_session_starts() {
        assert!(captured(Zeroizing::new(b"short".to_vec())).is_err());
        assert!(captured(Zeroizing::new(vec![b'x'; MIN_SECRET_LEN])).is_ok());

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("short"), "short\n").unwrap();
        assert!(FromFile::watch(dir.path().join("short"), FILE_REFRESH).is_err());
        let large = vec![b'x'; usize::try_from(MAX_FILE_LEN).unwrap() + 1];
        std::fs::write(dir.path().join("large"), large).unwrap();
        assert!(FromFile::watch(dir.path().join("large"), FILE_REFRESH).is_err());
    }

    #[test]
    fn a_missing_variable_is_named_but_a_value_never_shown() {
        let err = secret(
            &DevSecret {
                from_env: Some("WASH_DEV_SECRET_THAT_IS_NOT_SET".into()),
                ..entry()
            },
            Path::new("."),
        )
        .err()
        .unwrap();
        assert!(
            format!("{err:#}").contains("WASH_DEV_SECRET_THAT_IS_NOT_SET"),
            "{err:#}"
        );
    }

    #[test]
    fn a_file_source_is_refreshed_off_the_request_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "one-0123456789abcdef\n").unwrap();
        let source = FromFile::watch(path.clone(), Duration::from_millis(10)).unwrap();
        assert_eq!(
            source.current().unwrap().as_slice(),
            b"one-0123456789abcdef"
        );

        let eventually = |want: Result<&[u8], CredentialError>| {
            for _ in 0..500 {
                let got = source.current();
                if got.as_ref().map(|v| v.as_slice()).map_err(Clone::clone) == want {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the file source never showed {want:?}");
        };
        std::fs::write(&path, "two-0123456789abcdef").unwrap();
        eventually(Ok(b"two-0123456789abcdef"));
        std::fs::remove_file(&path).unwrap();
        eventually(Err(CredentialError::Unavailable));
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

    #[test]
    fn entries_are_camel_case_with_snake_case_aliases() {
        let yaml = r#"
dev:
  secrets:
    - env: API_TOKEN
      fromEnv: GITHUB_TOKEN
      destinations: [https://api.github.com]
      header: x-api-key
      routes: [GET /repos]
      placeholderPrefix: ghp_
    - env: OTHER
      from_file: token.txt
      destinations: [api.example.com:8443]
      query: key
      placeholder_prefix: sk-
"#;
        let config: crate::config::Config = serde_saphyr::from_str(yaml).unwrap();
        let secrets = config.dev().secrets;
        assert_eq!(
            secrets[0],
            DevSecret {
                env: "API_TOKEN".into(),
                from_env: Some("GITHUB_TOKEN".into()),
                destinations: vec!["https://api.github.com".into()],
                header: Some("x-api-key".into()),
                routes: vec!["GET /repos".into()],
                placeholder_prefix: Some("ghp_".into()),
                ..Default::default()
            }
        );
        assert_eq!(secrets[1].from_file, Some(PathBuf::from("token.txt")));
        assert_eq!(secrets[1].query.as_deref(), Some("key"));
        assert_eq!(secrets[1].placeholder_prefix.as_deref(), Some("sk-"));

        for unknown in ["headers: [x-api-key]", "fromEnvironment: X"] {
            let yaml = format!(
                "dev:\n  secrets:\n    - env: A\n      destinations: [h]\n      {unknown}\n"
            );
            assert!(
                serde_saphyr::from_str::<crate::config::Config>(&yaml).is_err(),
                "{unknown}"
            );
        }
    }
}
