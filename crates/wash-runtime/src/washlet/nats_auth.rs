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
mod tests {
    use super::*;

    #[test]
    fn a_value_and_its_file_are_refused_together() {
        let token = NatsAuth {
            token: Some("t".into()),
            token_file: Some("t".into()),
            ..Default::default()
        };
        assert!(token.into_connect_options().is_err());
        let password = NatsAuth {
            password: Some("p".into()),
            password_file: Some("p".into()),
            ..Default::default()
        };
        assert!(password.into_connect_options().is_err());
    }

    #[test]
    fn credentials_and_an_nkey_are_refused_together() {
        let auth = NatsAuth {
            credentials: Some("a.creds".into()),
            nkey: Some("a.nk".into()),
            ..Default::default()
        };
        assert!(auth.into_connect_options().is_err());
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
