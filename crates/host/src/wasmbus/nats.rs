use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_nats::ConnectOptions;
use nkeys::KeyPair;

/// Given the NATS authentication jwt, seed, and tls requirement, return
/// the proper [`async_nats::ConnectOptions`] to use for connecting.
///
/// Returns an error if only one of JWT or seed is specified, as we cannot
/// authenticate with only one of them.
pub(crate) fn connection_options(
    jwt: Option<&String>,
    seed: Option<&String>,
    require_tls: bool,
) -> Result<ConnectOptions> {
    match (jwt, seed) {
        (Some(jwt), Some(seed)) => {
            let kp = Arc::new(
                KeyPair::from_seed(seed).context("failed to construct key pair from seed")?,
            );
            Ok(ConnectOptions::with_jwt(jwt.to_string(), move |nonce| {
                let key_pair = kp.clone();
                async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
            }))
        }
        (Some(_), None) | (None, Some(_)) => {
            bail!("cannot authenticate if only one of jwt or seed is specified")
        }
        _ => Ok(ConnectOptions::new()),
    }
    .map(|opts| opts.require_tls(require_tls))
}
