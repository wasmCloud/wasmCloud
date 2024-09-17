//! Reusable types related to enabling consistent TLS ([webpki-roots]/[rustls-native-certs]) usage in downstream libraries.
//!
//! Downstream libraries can utilize this module to ensure a consistent set of CA roots and/or connectors.
//!
//! For example, when building a [`rustls::ClientConfig`]:
//!
//! ```rust
//! use rustls;
//! use wasmcloud_core::tls;
//!
//! let config = rustls::ClientConfig::builder()
//!     .with_root_certificates(rustls::RootCertStore {
//!         roots: tls::DEFAULT_ROOTS.roots.clone(),
//!     })
//!     .with_no_client_auth();
//!
//! # assert!(config.is_ok());
//! ```
//!
//! [webpki-roots]: https://crates.io/crates/webpki-roots
//! [rustls-native-certs]: https://crates.io/crates/rustls-native-certs

use std::{path::Path, sync::Arc};

use anyhow::{Context as _, Result};
use once_cell::sync::Lazy;

#[cfg(feature = "rustls-native-certs")]
pub static NATIVE_ROOTS: Lazy<Arc<[rustls::pki_types::CertificateDer<'static>]>> =
    Lazy::new(|| match rustls_native_certs::load_native_certs() {
        Ok(certs) => certs.into(),
        Err(err) => {
            tracing::warn!(?err, "failed to load native root certificate store");
            Arc::new([])
        }
    });

#[cfg(all(feature = "rustls-native-certs", feature = "oci"))]
pub static NATIVE_ROOTS_OCI: Lazy<Arc<[oci_distribution::client::Certificate]>> = Lazy::new(|| {
    NATIVE_ROOTS
        .iter()
        .map(|cert| oci_distribution::client::Certificate {
            encoding: oci_distribution::client::CertificateEncoding::Der,
            data: cert.to_vec(),
        })
        .collect()
});

#[cfg(all(feature = "rustls-native-certs", feature = "reqwest"))]
pub static NATIVE_ROOTS_REQWEST: Lazy<Arc<[reqwest::tls::Certificate]>> = Lazy::new(|| {
    NATIVE_ROOTS
        .iter()
        .filter_map(|cert| reqwest::tls::Certificate::from_der(cert.as_ref()).ok())
        .collect()
});

pub static DEFAULT_ROOTS: Lazy<Arc<rustls::RootCertStore>> = Lazy::new(|| {
    #[allow(unused_mut)]
    let mut ca = rustls::RootCertStore::empty();
    #[cfg(feature = "rustls-native-certs")]
    {
        let (added, ignored) = ca.add_parsable_certificates(NATIVE_ROOTS.iter().cloned());
        tracing::debug!(added, ignored, "loaded native root certificate store");
    }
    #[cfg(feature = "webpki-roots")]
    ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(ca)
});

pub static DEFAULT_CLIENT_CONFIG: Lazy<rustls::ClientConfig> = Lazy::new(|| {
    rustls::ClientConfig::builder()
        .with_root_certificates(Arc::clone(&DEFAULT_ROOTS))
        .with_no_client_auth()
});

#[cfg(feature = "hyper-rustls")]
pub static DEFAULT_HYPER_CONNECTOR: Lazy<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
> = Lazy::new(|| {
    hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(DEFAULT_CLIENT_CONFIG.clone())
        .https_or_http()
        .enable_all_versions()
        .build()
});

#[cfg(all(feature = "reqwest", feature = "rustls-native-certs"))]
pub static DEFAULT_REQWEST_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::ClientBuilder::default()
        .user_agent(REQWEST_USER_AGENT)
        .with_native_certificates()
        .build()
        .expect("failed to build HTTP client")
});

pub static REQWEST_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[cfg(feature = "rustls-native-certs")]
pub trait NativeRootsExt {
    fn with_native_certificates(self) -> Self;
}

#[cfg(all(feature = "reqwest", feature = "rustls-native-certs"))]
impl NativeRootsExt for reqwest::ClientBuilder {
    fn with_native_certificates(self) -> Self {
        NATIVE_ROOTS_REQWEST
            .iter()
            .cloned()
            .fold(self, reqwest::ClientBuilder::add_root_certificate)
    }
}

/// Attempt to load certificates from a given array of paths
pub fn load_certs_from_paths(
    paths: &[impl AsRef<Path>],
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    paths
        .iter()
        .map(read_certs_from_path)
        .flat_map(|result| match result {
            Ok(vec) => vec.into_iter().map(Ok).collect(),
            Err(er) => vec![Err(er)],
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Read certificates from a given path
///
/// At present this function only supports files -- directories will return an empty list
pub fn read_certs_from_path(
    path: impl AsRef<Path>,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let path = path.as_ref();
    // TODO(joonas): Support directories
    if !path.is_file() {
        return Ok(Vec::with_capacity(0));
    }
    let mut reader =
        std::io::BufReader::new(std::fs::File::open(path).with_context(|| {
            format!("failed to open file at provided path: {}", path.display())
        })?);
    Ok(rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?)
}
