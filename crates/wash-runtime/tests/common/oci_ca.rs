//! A private CA and localhost certificates for OCI trust tests.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, SystemTime};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

pub struct Ca(CertifiedIssuer<'static, KeyPair>);

impl Ca {
    pub fn new(name: &str) -> Self {
        let now = SystemTime::now();
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = (now - Duration::from_secs(3_600)).into();
        params.not_after = (now + Duration::from_secs(172_800)).into();
        Self(CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap())
    }

    pub fn pem(&self) -> String {
        self.0.pem()
    }

    pub fn server_config(&self) -> ServerConfig {
        let now = SystemTime::now();
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = (now - Duration::from_secs(3_600)).into();
        params.not_after = (now + Duration::from_secs(86_400)).into();
        let key = KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &self.0).unwrap();
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone(), self.0.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap()
    }
}
