use std::sync::Arc;

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

#[cfg(all(feature = "rustls-native-certs", feature = "oci-distribution"))]
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
        .with_native_certificates()
        .build()
        .expect("failed to build HTTP client")
});

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
