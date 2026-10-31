//! Client authentication for the connections [`super::connect_nats`] opens.

use std::path::PathBuf;

use zeroize::Zeroizing;

pub use crate::nats_credentials::AuthHook;
use crate::nats_credentials::{Credentials, Secret};

/// How a connection opened by [`connect_nats`](super::connect_nats)
/// authenticates.
///
/// The fields combine the way the fields of a NATS `CONNECT` do, except that
/// `credentials` and `nkey` both answer the server's nonce, so only one of them
/// may be set. Files are read on every connect rather than once, so a rotated
/// credential is presented on the next reconnect without a restart.
///
/// `#[non_exhaustive]`: build one with [`Default`] and the `with_*` methods
/// rather than a struct literal, so that adding a way to authenticate later is
/// not a breaking change for callers outside this crate.
#[derive(Clone, Default)]
#[non_exhaustive]
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
    /// The options to build a connection on: plain ones when there is nothing
    /// to present, so an unauthenticated connection is unchanged.
    ///
    /// [`connect_nats`](super::connect_nats) starts from these. An embedder
    /// that opens its own connection can start from them too, and keeps the
    /// rereading of every file on each connect. Anything to present is sent by
    /// an auth callback, which replaces credentials set on the result.
    pub fn into_connect_options(self) -> anyhow::Result<async_nats::ConnectOptions> {
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
        Credentials {
            credentials,
            jwt: None,
            nkey: nkey.map(Secret::File),
            token: value_or_file(token, token_file, "token")?,
            user,
            password: value_or_file(password, password_file, "password")?,
            hook,
        }
        .connect_options()
    }

    /// See [`NatsAuth::credentials`].
    #[must_use]
    pub fn with_credentials(mut self, path: impl Into<PathBuf>) -> Self {
        self.credentials = Some(path.into());
        self
    }

    /// See [`NatsAuth::nkey`].
    #[must_use]
    pub fn with_nkey(mut self, path: impl Into<PathBuf>) -> Self {
        self.nkey = Some(path.into());
        self
    }

    /// See [`NatsAuth::token`].
    #[must_use]
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// See [`NatsAuth::token_file`].
    #[must_use]
    pub fn with_token_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.token_file = Some(path.into());
        self
    }

    /// See [`NatsAuth::user`].
    #[must_use]
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// See [`NatsAuth::password`].
    #[must_use]
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// See [`NatsAuth::password_file`].
    #[must_use]
    pub fn with_password_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.password_file = Some(path.into());
        self
    }

    /// See [`NatsAuth::hook`].
    #[must_use]
    pub fn with_hook(mut self, hook: AuthHook) -> Self {
        self.hook = Some(hook);
        self
    }
}

fn value_or_file(
    value: Option<String>,
    file: Option<PathBuf>,
    what: &str,
) -> anyhow::Result<Option<Secret>> {
    match (value, file) {
        (Some(_), Some(_)) => {
            anyhow::bail!("a NATS connection takes a {what} or a {what} file, not both")
        }
        (Some(value), None) => Ok(Some(Secret::Value(Zeroizing::new(value)))),
        (None, Some(file)) => Ok(Some(Secret::File(file))),
        (None, None) => Ok(None),
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Each setter sets only its own field, the way a caller outside this
    /// crate builds one.
    #[test]
    fn each_setter_sets_its_field() {
        let auth = NatsAuth::default()
            .with_credentials("a.creds")
            .with_nkey("a.nk")
            .with_token("t")
            .with_token_file("t.file")
            .with_user("u")
            .with_password("p")
            .with_password_file("p.file")
            .with_hook(AuthHook::jwt_as_token());
        assert_eq!(auth.credentials, Some("a.creds".into()));
        assert_eq!(auth.nkey, Some("a.nk".into()));
        assert_eq!(auth.token.as_deref(), Some("t"));
        assert_eq!(auth.token_file, Some("t.file".into()));
        assert_eq!(auth.user.as_deref(), Some("u"));
        assert_eq!(auth.password.as_deref(), Some("p"));
        assert_eq!(auth.password_file, Some("p.file".into()));
        assert!(auth.hook.is_some());
    }

    #[test]
    fn a_value_and_its_file_are_refused_together() {
        let token = NatsAuth::default().with_token("t").with_token_file("t");
        assert!(token.into_connect_options().is_err());
        let password = NatsAuth::default()
            .with_user("u")
            .with_password("p")
            .with_password_file("p");
        assert!(password.into_connect_options().is_err());
    }

    #[test]
    fn credentials_and_an_nkey_are_refused_together() {
        let auth = NatsAuth::default()
            .with_credentials("a.creds")
            .with_nkey("a.nk");
        assert!(auth.into_connect_options().is_err());
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let auth = NatsAuth::default()
            .with_token("s3cr3t-token")
            .with_user("derek")
            .with_password("s3cr3t-pass");
        let debug = format!("{auth:?}");
        assert!(!debug.contains("s3cr3t"), "{debug}");
        assert!(debug.contains("derek"), "{debug}");
    }
}
