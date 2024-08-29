//! Configuration settings for [`HttpServerProvider`](crate::HttpServerProvider).
//! The "values" map in the component link definition may contain
//! one or more of the following keys,
//! which determine how the configuration is parsed.
//!
//! For the key...
///   `config_file`:       load configuration from file name.
///                      Interprets file as json or toml, based on file extension.
///   `config_b64`:        Configuration is a base64-encoded json string
///   `config_json`:       Configuration is a raw json string
///
/// If no configuration is provided, the default settings below will be used:
/// - TLS is disabled
/// - CORS allows all hosts(origins), most methods, and common headers
///   (see constants below).
/// - Default listener is bound to 127.0.0.1 port 8000.
///
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use base64::engine::Engine as _;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use http::Uri;
use serde::{de, de::Deserializer, de::Visitor, Deserialize, Serialize};
use tracing::{instrument, trace};
use unicase::UniCase;

const CORS_ALLOWED_ORIGINS: &[&str] = &[];
const CORS_ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "HEAD", "OPTIONS"];
const CORS_ALLOWED_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "content-language",
];
const CORS_EXPOSED_HEADERS: &[&str] = &[];
const CORS_DEFAULT_MAX_AGE_SECS: u64 = 300;

pub(crate) fn default_listen_address() -> SocketAddr {
    (Ipv4Addr::UNSPECIFIED, 8000).into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceSettings {
    /// Bind address
    #[serde(default = "default_listen_address")]
    pub address: SocketAddr,
    /// cache control options
    #[serde(default)]
    pub cache_control: Option<String>,
    /// Flag for read only mode
    #[serde(default)]
    pub readonly_mode: Option<bool>,
    // cors config
    pub cors_allowed_origins: Option<AllowedOrigins>,
    pub cors_allowed_headers: Option<AllowedHeaders>,
    pub cors_allowed_methods: Option<AllowedMethods>,
    pub cors_exposed_headers: Option<ExposedHeaders>,
    pub cors_max_age_secs: Option<u64>,
    // tls config
    #[serde(default)]
    /// path to server X.509 cert chain file. Must be PEM-encoded
    pub tls_cert_file: Option<String>,
    #[serde(default)]
    pub tls_priv_key_file: Option<String>,
    /// Rpc timeout - how long (milliseconds) to wait for component's response
    /// before returning a status 503 to the http client
    /// If not set, uses the system-wide rpc timeout
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    // DEPRECATED due to the nested struct being poorly supported by wasmCloud config
    #[deprecated(since = "0.22.0", note = "Use top-level fields instead")]
    #[serde(default)]
    pub tls: Tls,
    #[deprecated(since = "0.22.0", note = "Use top-level fields instead")]
    #[serde(default)]
    pub cors: Cors,
}

impl Default for ServiceSettings {
    fn default() -> ServiceSettings {
        #[allow(deprecated)]
        ServiceSettings {
            address: default_listen_address(),
            cors_allowed_origins: Some(AllowedOrigins::default()),
            cors_allowed_headers: Some(AllowedHeaders::default()),
            cors_allowed_methods: Some(AllowedMethods::default()),
            cors_exposed_headers: Some(ExposedHeaders::default()),
            cors_max_age_secs: Some(CORS_DEFAULT_MAX_AGE_SECS),
            tls_cert_file: None,
            tls_priv_key_file: None,
            timeout_ms: None,
            cache_control: None,
            readonly_mode: Some(false),
            tls: Tls::default(),
            cors: Cors::default(),
        }
    }
}

impl ServiceSettings {
    /// load settings from json, flattening nested fields
    fn from_json(data: &str) -> Result<Self, HttpServerError> {
        #[allow(deprecated)]
        serde_json::from_str(data)
            // For backwards compatibility, we can pull the values from the `tls` and `cors` fields
            // and merge them into the top-level fields.
            .map(|s: ServiceSettings| ServiceSettings {
                address: s.address,
                cache_control: s.cache_control,
                readonly_mode: s.readonly_mode,
                timeout_ms: s.timeout_ms,
                tls_cert_file: s.tls_cert_file.or(s.tls.cert_file),
                tls_priv_key_file: s.tls_priv_key_file.or(s.tls.priv_key_file),
                cors_allowed_origins: s.cors_allowed_origins.or(s.cors.allowed_origins),
                cors_allowed_headers: s.cors_allowed_headers.or(s.cors.allowed_headers),
                cors_allowed_methods: s.cors_allowed_methods.or(s.cors.allowed_methods),
                cors_exposed_headers: s.cors_exposed_headers.or(s.cors.exposed_headers),
                cors_max_age_secs: s.cors_max_age_secs.or(s.cors.max_age_secs),
                tls: Tls::default(),
                cors: Cors::default(),
            })
            .map_err(|e| HttpServerError::Settings(format!("invalid json: {e}")))
    }

    /// perform additional validation checks on settings.
    /// Several checks have already been done during deserialization.
    /// All errors found are combined into a single error message
    fn validate(&self) -> Result<(), HttpServerError> {
        let mut errors = Vec::new();
        // 1. make sure tls config is valid
        match (&self.tls_cert_file, &self.tls_priv_key_file) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                errors.push(
                    "for tls, both 'tls_cert_file' and 'tls_priv_key_file' must be set".to_string(),
                );
            }
            (Some(cert_file), Some(key_file)) => {
                for f in &[("cert_file", &cert_file), ("priv_key_file", &key_file)] {
                    let path: &Path = f.1.as_ref();
                    if !path.is_file() {
                        errors.push(format!(
                            "missing tls_{} '{}'{}",
                            f.0,
                            &path.display(),
                            if !path.is_absolute() {
                                " : perhaps you should make the path absolute"
                            } else {
                                ""
                            }
                        ));
                    }
                }
            }
        }
        if let Some(ref methods) = self.cors_allowed_methods {
            for m in &methods.0 {
                if http::Method::try_from(m.as_str()).is_err() {
                    errors.push(format!("invalid CORS method: '{m}'"));
                }
            }
        }
        if let Some(cache_control) = self.cache_control.as_ref() {
            if http::HeaderValue::from_str(cache_control).is_err() {
                errors.push(format!("Invalid Cache Control header : '{cache_control}'"));
            }
        }
        if !errors.is_empty() {
            Err(HttpServerError::Settings(format!(
                "\nInvalid httpserver settings: \n{}\n",
                errors.join("\n")
            )))
        } else {
            Ok(())
        }
    }
}

/// Errors generated by this HTTP server
#[derive(Debug, thiserror::Error)]
pub enum HttpServerError {
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("problem reading settings: {0}")]
    Settings(String),
}

/// Load settings provides a flexible means for loading configuration.
/// Return value is any structure with Deserialize, or for example, HashMap<String,String>
///   config_b64:  base64-encoded json string
///   config_json: raw json string
/// Also accept "address" (a string representing SocketAddr) and "port", a localhost port
/// If more than one key is provided, they are processed in the order above.
///   (later names override earlier names in the list)
///
#[instrument]
pub fn load_settings(
    default_address: SocketAddr,
    values: &HashMap<String, String>,
) -> Result<ServiceSettings, HttpServerError> {
    trace!("load settings");
    // Allow keys to be case insensitive, as an accommodation
    // for the lost souls who prefer sPoNgEbOb CaSe variable names.
    let values: HashMap<UniCase<&str>, &String> =
        HashMap::from_iter(values.iter().map(|(k, v)| (UniCase::new(k.as_str()), v)));

    if let Some(str) = values.get(&UniCase::new("config_b64")) {
        let bytes = BASE64_STANDARD_NO_PAD
            .decode(str)
            .map_err(|e| HttpServerError::Settings(format!("invalid base64 encoding: {e}")))?;
        return ServiceSettings::from_json(&String::from_utf8_lossy(&bytes));
    }

    if let Some(str) = values.get(&UniCase::new("config_json")) {
        return ServiceSettings::from_json(str);
    }

    let mut settings = ServiceSettings::default();

    // accept port, for compatibility with previous implementations
    if let Some(addr) = values.get(&UniCase::new("port")) {
        let port = addr
            .parse::<u16>()
            .map_err(|_| HttpServerError::InvalidParameter(format!("Invalid port: {addr}")))?;
        settings.address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
    }
    // accept address as value parameter
    settings.address = values
        .get(&UniCase::new("address"))
        .map(|addr| {
            SocketAddr::from_str(addr)
                .map_err(|_| HttpServerError::InvalidParameter(format!("invalid address: {addr}")))
        })
        .transpose()?
        .unwrap_or(default_address);

    // accept cache-control header values
    if let Some(cache_control) = values.get(&UniCase::new("cache_control")) {
        settings.cache_control = Some(cache_control.to_string());
    }
    // accept read only mode flag
    if let Some(readonly_mode) = values.get(&UniCase::new("readonly_mode")) {
        settings.readonly_mode = Some(readonly_mode.to_string().parse().unwrap_or(false));
    }
    // accept timeout_ms flag
    if let Some(Ok(timeout_ms)) = values.get(&UniCase::new("timeout_ms")).map(|s| s.parse()) {
        settings.timeout_ms = Some(timeout_ms)
    }

    // TLS
    if let Some(tls_cert_file) = values.get(&UniCase::new("tls_cert_file")) {
        settings.tls_cert_file = Some(tls_cert_file.to_string());
    }
    if let Some(tls_priv_key_file) = values.get(&UniCase::new("tls_priv_key_file")) {
        settings.tls_priv_key_file = Some(tls_priv_key_file.to_string());
    }

    // CORS
    if let Some(cors_allowed_origins) = values.get(&UniCase::new("cors_allowed_origins")) {
        let origins: Vec<CorsOrigin> = serde_json::from_str(cors_allowed_origins)
            .map_err(|e| HttpServerError::Settings(format!("invalid cors_allowed_origins: {e}")))?;
        settings.cors_allowed_origins = Some(AllowedOrigins(origins));
    }
    if let Some(cors_allowed_headers) = values.get(&UniCase::new("cors_allowed_headers")) {
        let headers: Vec<String> = serde_json::from_str(cors_allowed_headers)
            .map_err(|e| HttpServerError::Settings(format!("invalid cors_allowed_headers: {e}")))?;
        settings.cors_allowed_headers = Some(AllowedHeaders(headers));
    }
    if let Some(cors_allowed_methods) = values.get(&UniCase::new("cors_allowed_methods")) {
        let methods: Vec<String> = serde_json::from_str(cors_allowed_methods)
            .map_err(|e| HttpServerError::Settings(format!("invalid cors_allowed_methods: {e}")))?;
        settings.cors_allowed_methods = Some(AllowedMethods(methods));
    }
    if let Some(cors_exposed_headers) = values.get(&UniCase::new("cors_exposed_headers")) {
        let headers: Vec<String> = serde_json::from_str(cors_exposed_headers)
            .map_err(|e| HttpServerError::Settings(format!("invalid cors_exposed_headers: {e}")))?;
        settings.cors_exposed_headers = Some(ExposedHeaders(headers));
    }
    if let Some(cors_max_age_secs) = values.get(&UniCase::new("cors_max_age_secs")) {
        let max_age_secs: u64 = cors_max_age_secs.parse().map_err(|_| {
            HttpServerError::InvalidParameter("Invalid cors_max_age_secs".to_string())
        })?;
        settings.cors_max_age_secs = Some(max_age_secs);
    }

    settings.validate()?;
    Ok(settings)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tls {
    /// path to server X.509 cert chain file. Must be PEM-encoded
    pub cert_file: Option<String>,
    pub priv_key_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cors {
    pub allowed_origins: Option<AllowedOrigins>,
    pub allowed_headers: Option<AllowedHeaders>,
    pub allowed_methods: Option<AllowedMethods>,
    pub exposed_headers: Option<ExposedHeaders>,
    pub max_age_secs: Option<u64>,
}

impl Default for Cors {
    fn default() -> Self {
        Cors {
            allowed_origins: Some(AllowedOrigins::default()),
            allowed_headers: Some(AllowedHeaders::default()),
            allowed_methods: Some(AllowedMethods::default()),
            exposed_headers: Some(ExposedHeaders::default()),
            max_age_secs: Some(CORS_DEFAULT_MAX_AGE_SECS),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CorsOrigin(String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedOrigins(Vec<CorsOrigin>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedHeaders(Vec<String>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedMethods(Vec<String>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExposedHeaders(Vec<String>);

impl<'de> Deserialize<'de> for CorsOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CorsOriginVisitor;
        impl<'de> Visitor<'de> for CorsOriginVisitor {
            type Value = CorsOrigin;

            fn expecting(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
                write!(fmt, "an origin in format http[s]://example.com[:3000]",)
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                CorsOrigin::from_str(v).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(CorsOriginVisitor)
    }
}

impl FromStr for CorsOrigin {
    type Err = std::io::Error;

    fn from_str(origin: &str) -> Result<Self, Self::Err> {
        let uri = Uri::from_str(origin).map_err(|invalid_uri| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("Invalid uri: {origin}.\n{invalid_uri}"),
            )
        })?;
        if let Some(s) = uri.scheme_str() {
            if s != "http" && s != "https" {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "Cors origin invalid schema {}, only [http] and [https] are supported: ",
                        uri.scheme_str().unwrap()
                    ),
                ));
            }
        } else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "Cors origin missing schema, only [http] or [https] are supported",
            ));
        }

        if let Some(p) = uri.path_and_query() {
            if p.as_str() != "/" {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("Invalid value {} in cors schema.", p.as_str()),
                ));
            }
        }
        Ok(CorsOrigin(origin.trim_end_matches('/').to_owned()))
    }
}

impl AsRef<str> for CorsOrigin {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for AllowedOrigins {
    type Target = Vec<CorsOrigin>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for AllowedOrigins {
    fn default() -> Self {
        AllowedOrigins(
            CORS_ALLOWED_ORIGINS
                .iter()
                .map(|s| CorsOrigin((*s).to_string()))
                .collect::<Vec<_>>(),
        )
    }
}

impl Deref for AllowedHeaders {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for AllowedHeaders {
    fn default() -> Self {
        AllowedHeaders(from_defaults(CORS_ALLOWED_HEADERS))
    }
}

impl Default for AllowedMethods {
    fn default() -> Self {
        AllowedMethods(from_defaults(CORS_ALLOWED_METHODS))
    }
}

impl Deref for AllowedMethods {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for ExposedHeaders {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for ExposedHeaders {
    fn default() -> Self {
        ExposedHeaders(
            CORS_EXPOSED_HEADERS
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Connect,
    Patch,
    Trace,
}

impl FromStr for HttpMethod {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "PUT" => Ok(Self::Put),
            "POST" => Ok(Self::Post),
            "DELETE" => Ok(Self::Delete),
            "HEAD" => Ok(Self::Head),
            "OPTIONS" => Ok(Self::Options),
            "CONNECT" => Ok(Self::Connect),
            "PATCH" => Ok(Self::Patch),
            "TRACE" => Ok(Self::Trace),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{s} is not a valid http method"),
            )),
        }
    }
}

/// convert array of &str into array of T if T is From<&str>
fn from_defaults<'d, T>(d: &[&'d str]) -> Vec<T>
where
    T: std::convert::From<&'d str>,
{
    // unwrap ok here bacause this is only used for default values
    d.iter().map(|s| T::from(*s)).collect::<Vec<_>>()
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use crate::settings::{CorsOrigin, ServiceSettings};

    const GOOD_ORIGINS: &[&str] = &[
        // origins that should be parsed correctly
        "https://www.example.com",
        "https://www.example.com:1000",
        "http://localhost",
        "http://localhost:8080",
        "http://127.0.0.1",
        "http://127.0.0.1:8080",
        "https://:8080",
    ];

    const BAD_ORIGINS: &[&str] = &[
        // invalid origin syntax
        "ftp://www.example.com", // only http,https allowed
        "localhost",
        "127.0.0.1",
        "127.0.0.1:8080",
        ":8080",
        "/path/file.txt",
        "http:",
        "https://",
    ];

    #[test]
    fn settings_init() {
        let s = ServiceSettings::default();
        assert!(s.address.is_ipv4());
        assert!(s.cors_allowed_methods.is_some());
        assert!(s.cors_allowed_origins.is_some());
        assert!(s.cors_allowed_origins.unwrap().0.is_empty());
    }

    #[test]
    fn settings_json() {
        let json = r#"{
        "cors": {
            "allowed_headers": [ "X-Cookies" ]
         }
         }"#;

        let s = ServiceSettings::from_json(json).expect("parse_json");
        assert_eq!(s.cors_allowed_headers.as_ref().unwrap().0.len(), 1);
        assert_eq!(
            s.cors_allowed_headers.as_ref().unwrap().0.first().unwrap(),
            "X-Cookies"
        );
    }

    #[test]
    fn origins_deserialize() {
        // test CorsOrigin
        for valid in GOOD_ORIGINS {
            let o = serde_json::from_value::<CorsOrigin>(serde_json::Value::String(
                (*valid).to_string(),
            ));
            assert!(o.is_ok(), "from_value '{valid}'");

            // test as_ref()
            assert_eq!(&o.unwrap().0, valid);
        }
    }

    #[test]
    fn origins_from_str() {
        // test CorsOrigin
        for &valid in GOOD_ORIGINS {
            let o = CorsOrigin::from_str(valid);
            println!("{valid}: {o:?}");
            assert!(o.is_ok(), "from_str '{valid}'");

            // test as_ref()
            assert_eq!(&o.unwrap().0, valid);
        }
    }

    #[test]
    fn origins_negative() {
        for bad in BAD_ORIGINS {
            let o =
                serde_json::from_value::<CorsOrigin>(serde_json::Value::String((*bad).to_string()));
            println!("{bad}: {o:?}");
            assert!(o.is_err(), "from_value '{bad}' (expect err)");

            let o = serde_json::from_str::<CorsOrigin>(bad);
            println!("{bad}: {o:?}");
            assert!(o.is_err(), "from_str '{bad}' (expect err)");
        }
    }
}
