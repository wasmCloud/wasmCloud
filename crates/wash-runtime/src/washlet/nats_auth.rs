//! Client authentication for the connections [`super::connect_nats`] opens.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use zeroize::Zeroizing;

/// How a connection opened by [`connect_nats`](super::connect_nats)
/// authenticates.
///
/// The fields combine the way the fields of a NATS `CONNECT` do, except that
/// `credentials` and `nkey` both answer the server's nonce, so only one of them
/// may be set. Files are read on every connect rather than once, so a rotated
/// credential is presented on the next reconnect without a restart.
#[derive(Clone, Default)]
pub struct NatsAuth {
    /// A NATS `.creds` file: a user JWT and the nkey seed that signs for it.
    pub credentials: Option<PathBuf>,
    /// A file holding an nkey seed, bare or in the decorated form `nsc` and
    /// `nk` write.
    pub nkey: Option<PathBuf>,
    /// Sent as `auth_token`.
    pub token: Option<String>,
    /// A file holding the `auth_token`, in place of `token`.
    pub token_file: Option<PathBuf>,
    /// Sent as `user`.
    pub user: Option<String>,
    /// Sent as `pass`.
    pub password: Option<String>,
    /// A file holding the `pass`, in place of `password`.
    pub password_file: Option<PathBuf>,
    /// Adjusts what the client presents after the fields above are applied.
    pub hook: Option<AuthHook>,
}

impl NatsAuth {
    fn is_empty(&self) -> bool {
        let Self {
            credentials,
            nkey,
            token,
            token_file,
            user,
            password,
            password_file,
            hook,
        } = self;
        credentials.is_none()
            && nkey.is_none()
            && token.is_none()
            && token_file.is_none()
            && user.is_none()
            && password.is_none()
            && password_file.is_none()
            && hook.is_none()
    }

    /// The options to build a connection on: plain ones when there is nothing
    /// to present, so an unauthenticated connection is unchanged.
    pub(super) fn connect_options(self) -> anyhow::Result<async_nats::ConnectOptions> {
        anyhow::ensure!(
            self.credentials.is_none() || self.nkey.is_none(),
            "a NATS connection takes a credentials file or an nkey seed, not both"
        );
        anyhow::ensure!(
            self.token.is_none() || self.token_file.is_none(),
            "a NATS connection takes a token or a token file, not both"
        );
        anyhow::ensure!(
            self.password.is_none() || self.password_file.is_none(),
            "a NATS connection takes a password or a password file, not both"
        );
        if self.is_empty() {
            return Ok(async_nats::ConnectOptions::new());
        }
        let auth = Arc::new(Presenter::from(self));
        Ok(async_nats::ConnectOptions::with_auth_callback(
            move |nonce| {
                let auth = Arc::clone(&auth);
                async move {
                    auth.present(&nonce)
                        .await
                        .map_err(|e| async_nats::AuthError::new(format!("{e:#}")))
                }
            },
        ))
    }
}

/// A [`NatsAuth`] plus the files it reads, kept across every connect of one
/// connection.
struct Presenter {
    auth: NatsAuth,
    credentials: Option<SecretFile>,
    nkey: Option<SecretFile>,
    token: Option<SecretFile>,
    password: Option<SecretFile>,
}

impl From<NatsAuth> for Presenter {
    fn from(auth: NatsAuth) -> Self {
        Self {
            credentials: auth.credentials.clone().map(SecretFile::new),
            nkey: auth.nkey.clone().map(SecretFile::new),
            token: auth.token_file.clone().map(SecretFile::new),
            password: auth.password_file.clone().map(SecretFile::new),
            auth,
        }
    }
}

impl Presenter {
    /// What to send in `CONNECT` for a server that issued `nonce`.
    async fn present(&self, nonce: &[u8]) -> anyhow::Result<async_nats::Auth> {
        // Read together, so files on one stalled mount cost one READ_WAIT.
        let (credentials, nkey, token, password) = tokio::try_join!(
            read_file(&self.credentials, "credentials", credentials_key),
            read_file(&self.nkey, "nkey seed", nkey_key),
            read_file(&self.token, "token", secret_line),
            read_file(&self.password, "password", secret_line),
        )?;
        let mut auth = async_nats::Auth::new();
        if let Some((jwt, key)) = credentials {
            auth.jwt = Some(jwt);
            auth.signature = Some(sign(&key, nonce)?);
        }
        if let Some(key) = nkey {
            auth.nkey = Some(key.public_key());
            auth.signature = Some(sign(&key, nonce)?);
        }
        auth.token = token.or_else(|| self.auth.token.clone());
        auth.username = self.auth.user.clone();
        auth.password = password.or_else(|| self.auth.password.clone());
        if let Some(hook) = &self.auth.hook {
            (hook.0)(&mut auth)?;
        }
        Ok(auth)
    }
}

impl std::fmt::Debug for NatsAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("NatsAuth")
            .field("credentials", &self.credentials)
            .field("nkey", &self.nkey)
            .field("token", &redacted(&self.token))
            .field("token_file", &self.token_file)
            .field("user", &self.user)
            .field("password", &redacted(&self.password))
            .field("password_file", &self.password_file)
            .field("hook", &self.hook)
            .finish()
    }
}

/// Runs on every connect with the [`async_nats::Auth`] that [`NatsAuth`]'s
/// fields produced, and may change any of it before it is sent.
///
/// This is for a server that wants credentials in a shape the fields cannot
/// express. An auth callout behind a server without trusted operators never
/// sees the `jwt` a client sends, for example, so a deployment may agree to
/// have clients repeat it as their token:
///
/// ```
/// # use wash_runtime::washlet::AuthHook;
/// let hook = AuthHook::new(|auth| {
///     auth.token = auth.jwt.clone();
///     Ok(())
/// });
/// ```
///
/// An error refuses the connection attempt, and is reported as an
/// authentication failure.
#[derive(Clone)]
pub struct AuthHook(Arc<AuthCallback>);

type AuthCallback = dyn Fn(&mut async_nats::Auth) -> anyhow::Result<()> + Send + Sync;

impl AuthHook {
    pub fn new(
        hook: impl Fn(&mut async_nats::Auth) -> anyhow::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(hook))
    }
}

impl std::fmt::Debug for AuthHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthHook")
    }
}

/// How long a connect waits on a file read. Shorter than async-nats' default
/// 5s connection timeout, so a stalled read is reported rather than dropped.
const READ_WAIT: Duration = Duration::from_secs(2);

/// A file reread on every connect, with at most one read in flight.
///
/// A detached worker lets runtime shutdown finish even if a read stalls.
/// Reconnects share the pending read and use the last contents that parsed
/// while the file stalls, fails to read, or does not parse.
struct SecretFile {
    path: PathBuf,
    state: tokio::sync::Mutex<ReadState>,
}

#[derive(Default)]
struct ReadState {
    pending: Option<PendingRead>,
    last_read: Option<Zeroizing<String>>,
    /// The failure last logged, so a lasting one is logged once.
    last_failure: Option<String>,
}

struct PendingRead {
    result: tokio::sync::oneshot::Receiver<std::io::Result<String>>,
    _worker: std::thread::JoinHandle<()>,
}

impl PendingRead {
    fn start(path: PathBuf) -> std::io::Result<Self> {
        let (sender, result) = tokio::sync::oneshot::channel();
        let worker = std::thread::Builder::new()
            .name("nats-secret-read".into())
            .spawn(move || {
                let _ = sender.send(std::fs::read_to_string(path));
            })?;
        Ok(Self {
            result,
            _worker: worker,
        })
    }
}

impl SecretFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: Default::default(),
        }
    }

    /// The file's contents as `parse` reads them, or the last contents that
    /// parsed while the file stalls, fails to read, or does not parse.
    async fn read<T>(&self, what: &str, parse: fn(&str) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut guard = self.state.lock().await;
        let state = &mut *guard;
        let failure = match self.wait(state).await {
            Ok(Some(contents)) => match parse(&contents) {
                Ok(value) => {
                    state.last_read = Some(contents);
                    state.last_failure = None;
                    return Ok(value);
                }
                Err(e) => e.context(format!("invalid NATS {what} file {}", self.path.display())),
            },
            Ok(None) => {
                anyhow::anyhow!("timed out reading NATS {what} file {}", self.path.display())
            }
            Err(e) => e.context(failed(&self.path, what)),
        };
        let Some(contents) = &state.last_read else {
            return Err(failure);
        };
        let failure = format!("{failure:#}");
        if state.last_failure.as_ref() != Some(&failure) {
            tracing::warn!(error = %failure, "presenting the last NATS {what} file contents that parsed");
            state.last_failure = Some(failure);
        }
        parse(contents)
    }

    /// The pending read's result, starting one if none is in flight, or
    /// `None` while it is still stalled after [`READ_WAIT`].
    async fn wait(&self, state: &mut ReadState) -> anyhow::Result<Option<Zeroizing<String>>> {
        let pending = match &mut state.pending {
            Some(pending) => pending,
            None => state.pending.insert(
                PendingRead::start(self.path.clone()).context("failed to start the file reader")?,
            ),
        };
        let Ok(read) = tokio::time::timeout(READ_WAIT, &mut pending.result).await else {
            return Ok(None);
        };
        state.pending = None;
        let contents = read.context("the file reader panicked")??;
        Ok(Some(Zeroizing::new(contents)))
    }
}

/// `file` as `parse` reads it, when there is one.
async fn read_file<T>(
    file: &Option<SecretFile>,
    what: &str,
    parse: fn(&str) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    match file {
        Some(file) => file.read(what, parse).await.map(Some),
        None => Ok(None),
    }
}

/// A `.creds` file's user JWT, and the key that signs for it.
fn credentials_key(contents: &str) -> anyhow::Result<(String, nkeys::KeyPair)> {
    let (jwt, seed) = parse_credentials(contents)?;
    Ok((jwt.to_string(), key_pair(seed)?))
}

/// The key in an nkey seed file, bare or decorated.
fn nkey_key(contents: &str) -> anyhow::Result<nkeys::KeyPair> {
    let seed = decorated_values(contents)
        .into_iter()
        .find(|value| value.starts_with('S'))
        .unwrap_or_else(|| contents.trim());
    key_pair(seed)
}

/// A token or password file's contents, less one LF or CRLF line ending.
fn secret_line(contents: &str) -> anyhow::Result<String> {
    let secret = match contents.strip_suffix('\n') {
        Some(line) => line.strip_suffix('\r').unwrap_or(line),
        None => contents,
    };
    anyhow::ensure!(!secret.is_empty(), "the file is empty");
    Ok(secret.to_string())
}

fn failed(path: &Path, what: &str) -> String {
    format!("failed to read NATS {what} file {}", path.display())
}

fn key_pair(seed: &str) -> anyhow::Result<nkeys::KeyPair> {
    // nkeys' errors name the problem, never the seed.
    nkeys::KeyPair::from_seed(seed).map_err(anyhow::Error::new)
}

fn sign(key: &nkeys::KeyPair, nonce: &[u8]) -> anyhow::Result<Vec<u8>> {
    key.sign(nonce)
        .map_err(anyhow::Error::new)
        .context("failed to sign the NATS server nonce")
}

/// The user JWT and nkey seed of a `.creds` file, in the order the file holds
/// them.
fn parse_credentials(contents: &str) -> anyhow::Result<(&str, &str)> {
    let mut values = decorated_values(contents).into_iter();
    let jwt = values.next().context("no user JWT block")?;
    let seed = values.next().context("no nkey seed block")?;
    Ok((jwt, seed))
}

/// Each value framed by a pair of `-----BEGIN …-----` / `------END …------`
/// lines, the way NATS tools decorate JWTs and seeds.
fn decorated_values(contents: &str) -> Vec<&str> {
    let is_marker = |line: &str| line.len() > 6 && line.starts_with("---") && line.ends_with("---");
    let lines: Vec<&str> = contents.lines().map(str::trim).collect();
    lines
        .windows(3)
        .filter_map(|window| match window {
            [start, value, end]
                if is_marker(start) && !value.is_empty() && !is_marker(value) && is_marker(end) =>
            {
                Some(*value)
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn creds_file(jwt: &str, seed: &str) -> String {
        format!(
            "-----BEGIN NATS USER JWT-----\n{jwt}\n------END NATS USER JWT------\n\n\
             ************************* IMPORTANT *************************\n\
             NKEY Seed printed below can be used to sign and prove identity.\n\n\
             -----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n\n\
             *************************************************************\n"
        )
    }

    #[test]
    fn credentials_parse_in_file_order() -> anyhow::Result<()> {
        let seed = nkeys::KeyPair::new_user().seed()?;
        let contents = creds_file("eyJhbGciOi.payload.sig", &seed);
        assert_eq!(
            parse_credentials(&contents)?,
            ("eyJhbGciOi.payload.sig", seed.as_str())
        );
        assert!(parse_credentials("eyJhbGciOi.payload.sig\n").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn the_seed_signs_the_nonce_and_the_hook_runs_last() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let key = nkeys::KeyPair::new_user();
        let path = dir.path().join("user.creds");
        std::fs::write(&path, creds_file("the.user.jwt", &key.seed()?))?;
        let auth = NatsAuth {
            credentials: Some(path),
            token: Some("ignored".into()),
            hook: Some(AuthHook::new(|auth| {
                auth.token = auth.jwt.clone();
                Ok(())
            })),
            ..Default::default()
        };

        let presented = Presenter::from(auth).present(b"nonce").await?;
        assert_eq!(presented.jwt.as_deref(), Some("the.user.jwt"));
        assert_eq!(presented.token.as_deref(), Some("the.user.jwt"));
        key.verify(b"nonce", presented.signature.as_deref().unwrap_or_default())?;
        Ok(())
    }

    #[tokio::test]
    async fn an_nkey_file_may_be_bare_or_decorated() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let key = nkeys::KeyPair::new_user();
        let seed = key.seed()?;
        for (name, contents) in [
            ("bare.nk", format!("{seed}\n")),
            (
                "decorated.nk",
                format!("-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n"),
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, contents)?;
            let auth = NatsAuth {
                nkey: Some(path),
                ..Default::default()
            };
            let presented = Presenter::from(auth).present(b"nonce").await?;
            assert_eq!(presented.nkey, Some(key.public_key()), "{name}");
            key.verify(b"nonce", presented.signature.as_deref().unwrap_or_default())?;
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stalled_read_is_shared_and_presents_the_last_contents() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("token");
        std::fs::write(&path, "first")?;
        let file = SecretFile::new(path.clone());
        assert_eq!(file.read("token", secret_line).await?, "first");

        // Opening a FIFO that has no writer blocks, the way a hung mount does.
        std::fs::remove_file(&path)?;
        anyhow::ensure!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()?
                .success()
        );
        let pending = |file: &SecretFile| {
            file.state.try_lock().ok().and_then(|state| {
                state
                    .pending
                    .as_ref()
                    .map(|read| read._worker.thread().id())
            })
        };
        assert_eq!(file.read("token", secret_line).await?, "first");
        let stalled = pending(&file).context("no read in flight")?;
        assert_eq!(file.read("token", secret_line).await?, "first");
        assert_eq!(pending(&file), Some(stalled));

        let writer = tokio::task::spawn_blocking(move || std::fs::write(path, "second"));
        assert_eq!(file.read("token", secret_line).await?, "second");
        writer.await??;
        assert_eq!(pending(&file), None);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_read_presents_the_last_contents() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("token");
        std::fs::write(&path, "first")?;
        let file = SecretFile::new(path.clone());
        assert_eq!(file.read("token", secret_line).await?, "first");

        std::fs::remove_file(&path)?;
        assert_eq!(file.read("token", secret_line).await?, "first");
        std::fs::write(&path, "second")?;
        assert_eq!(file.read("token", secret_line).await?, "second");

        let missing = SecretFile::new(dir.path().join("missing"));
        let error = missing
            .read("token", secret_line)
            .await
            .err()
            .context("missing file read")?;
        assert!(
            format!("{error:#}").contains("failed to read NATS token file"),
            "{error:#}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_stalled_read_does_not_block_runtime_shutdown() -> anyhow::Result<()> {
        const CHILD_PATH: &str = "WASH_NATS_SECRET_SHUTDOWN_CHILD";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let file = SecretFile::new(path.into());
            let error = runtime
                .block_on(file.read("token", secret_line))
                .err()
                .context("stalled read did not time out")?;
            assert!(error.to_string().contains("timed out reading"), "{error:#}");
            drop(file);
            drop(runtime);
            return Ok(());
        }

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("token");
        anyhow::ensure!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()?
                .success()
        );
        let (_, module) = module_path!().split_once("::").context("no test module")?;
        let test = format!("{module}::a_stalled_read_does_not_block_runtime_shutdown");
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", &test, "--nocapture"])
            .env(CHILD_PATH, path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait()?.is_some() {
                let output = child.wait_with_output()?;
                anyhow::ensure!(
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                    "shutdown child failed: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                anyhow::bail!("stalled secret read blocked runtime shutdown");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[tokio::test]
    async fn token_and_password_files_lose_one_line_ending() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let token = dir.path().join("token");
        let password = dir.path().join("password");
        std::fs::write(&token, " the token \r\n")?;
        std::fs::write(&password, "the password\n\n")?;
        let presenter = Presenter::from(NatsAuth {
            token_file: Some(token.clone()),
            user: Some("derek".into()),
            password_file: Some(password),
            ..Default::default()
        });
        let presented = presenter.present(b"nonce").await?;
        assert_eq!(presented.token.as_deref(), Some(" the token "));
        assert_eq!(presented.password.as_deref(), Some("the password\n"));

        std::fs::write(&token, "\n")?;
        let presented = presenter.present(b"nonce").await?;
        assert_eq!(presented.token.as_deref(), Some(" the token "));

        let empty = Presenter::from(NatsAuth {
            token_file: Some(token),
            ..Default::default()
        });
        let error = empty.present(b"nonce").await.err().context("empty token")?;
        assert!(format!("{error:#}").contains("is empty"), "{error:#}");
        Ok(())
    }

    #[tokio::test]
    async fn a_file_that_does_not_parse_presents_the_last_one_that_did() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let key = nkeys::KeyPair::new_user();
        let path = dir.path().join("user.creds");
        let contents = creds_file("the.user.jwt", &key.seed()?);
        std::fs::write(&path, &contents)?;
        let presenter = Presenter::from(NatsAuth {
            credentials: Some(path.clone()),
            ..Default::default()
        });
        assert!(presenter.present(b"nonce").await.is_ok());

        // A rewrite caught halfway through.
        std::fs::write(&path, &contents[..contents.len() / 2])?;
        let presented = presenter.present(b"nonce").await?;
        assert_eq!(presented.jwt.as_deref(), Some("the.user.jwt"));
        key.verify(b"nonce", presented.signature.as_deref().unwrap_or_default())?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_files_cost_one_read_wait() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let paths = ["user.creds", "token", "password"].map(|name| dir.path().join(name));
        for path in &paths {
            anyhow::ensure!(
                std::process::Command::new("mkfifo")
                    .arg(path)
                    .status()?
                    .success()
            );
        }
        let [credentials, token, password] = paths.clone();
        let presenter = Presenter::from(NatsAuth {
            credentials: Some(credentials),
            token_file: Some(token),
            user: Some("derek".into()),
            password_file: Some(password),
            ..Default::default()
        });
        let started = std::time::Instant::now();
        assert!(presenter.present(b"nonce").await.is_err());
        assert!(started.elapsed() < READ_WAIT * 2, "{:?}", started.elapsed());

        // Release the stalled readers.
        for path in paths {
            tokio::task::spawn_blocking(move || std::fs::write(path, "")).await??;
        }
        Ok(())
    }

    #[test]
    fn a_value_and_its_file_are_refused_together() {
        let token = NatsAuth {
            token: Some("t".into()),
            token_file: Some("t".into()),
            ..Default::default()
        };
        assert!(token.connect_options().is_err());
        let password = NatsAuth {
            password: Some("p".into()),
            password_file: Some("p".into()),
            ..Default::default()
        };
        assert!(password.connect_options().is_err());
    }

    #[test]
    fn credentials_and_an_nkey_are_refused_together() {
        let auth = NatsAuth {
            credentials: Some("a.creds".into()),
            nkey: Some("a.nk".into()),
            ..Default::default()
        };
        assert!(auth.connect_options().is_err());
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let auth = NatsAuth {
            token: Some("s3cr3t-token".into()),
            user: Some("derek".into()),
            password: Some("s3cr3t-pass".into()),
            ..Default::default()
        };
        let debug = format!("{auth:?}");
        assert!(!debug.contains("s3cr3t"), "{debug}");
        assert!(debug.contains("derek"), "{debug}");
    }
}
