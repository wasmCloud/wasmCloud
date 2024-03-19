//! Configuration settings for HttpServer.
//! The "values" map in the actor link definition may contain
//! one or more of the following keys,
//! which determine how the configuration is parsed.
//!
//! For the key...
///   config_file:       load configuration from file name.
///                      Interprets file as json or toml, based on file extension.
///   config_b64:        Configuration is a base64-encoded json string
///   config_json:       Configuration is a raw json string
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

use crate::HttpServerError;

const DEFAULT_ADDR: &str = "127.0.0.1:8000";
const DEFAULT_LOG_LEVEL: LogLevel = LogLevel::Debug;

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
// Maximum content length. Can be overridden in settings or link definition
// Syntax: number, or number followed by 'K', 'M', or 'G'
// Default value is 100M (100*1024*1024)
pub const DEFAULT_MAX_CONTENT_LEN: u64 = 100 * 1024 * 1024;
// max possible value of content length. If sending to wasm32, memory is limited to 2GB,
// practically this should be quite a bit smaller. Setting to 1GB for now.
pub const CONTENT_LEN_LIMIT: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceSettings {
    /// Bind address
    #[serde(default)]
    pub address: Option<SocketAddr>,

    /// tls config
    #[serde(default)]
    pub tls: Tls,

    /// cors config
    #[serde(default)]
    pub cors: Cors,

    /// logging
    #[serde(default)]
    pub log: Log,

    /// Rpc timeout - how long (milliseconds) to wait for actor's response
    /// before returning a status 503 to the http client
    /// If not set, uses the system-wide rpc timeout
    #[serde(default)]
    pub timeout_ms: Option<u64>,

    /// cache control options
    pub cache_control: Option<String>,

    /// Flag for read only mode
    pub readonly_mode: Option<bool>,

    /// Max content length. Default "10m" (10MiB = 10485760 bytes)
    /// Can be overridden by link def value max_content_len
    /// Accepts number (bytes), or number with suffix 'k', 'm', or 'g', (upper or lower case)
    /// representing multiples of 1024. For example,
    /// - "500" = 5000 bytes,
    /// - "5k" = 5 * 1024 bytes,
    /// - "5m" = 5 * 1024*1024 bytes,
    /// - "1g" = 1024*1024*1024 bytes
    /// The value may not be higher than i32::MAX
    pub max_content_len: Option<String>,

    /// capture any other configuration values
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

impl Default for ServiceSettings {
    fn default() -> ServiceSettings {
        ServiceSettings {
            address: Some(SocketAddr::from_str(DEFAULT_ADDR).unwrap()),
            tls: Tls::default(),
            cors: Cors::default(),
            log: Log::default(),
            timeout_ms: None,
            cache_control: None,
            readonly_mode: Some(false),
            max_content_len: Some(DEFAULT_MAX_CONTENT_LEN.to_string()),
            extra: Default::default(),
        }
    }
}

macro_rules! merge {
    ( $self:ident, $other: ident, $( $field:ident),+ ) => {
        $(
            if $other.$field.is_some() {
                $self.$field = $other.$field;
            }
        )*
    };
}

impl ServiceSettings {
    /// load Settings from a file with .toml or .json extension
    fn from_file<P: AsRef<Path>>(fpath: P) -> Result<Self, HttpServerError> {
        let data = std::fs::read_to_string(&fpath).map_err(|e| {
            HttpServerError::Settings(format!("reading file {}: {}", &fpath.as_ref().display(), e))
        })?;
        if let Some(ext) = fpath.as_ref().extension() {
            let ext = ext.to_string_lossy();
            match ext.as_ref() {
                "json" => ServiceSettings::from_json(&data),
                "toml" => ServiceSettings::from_toml(&data),
                _ => Err(HttpServerError::Settings(format!(
                    "unrecognized extension {}",
                    ext
                ))),
            }
        } else {
            Err(HttpServerError::Settings(format!(
                "unrecognized file type {}",
                &fpath.as_ref().display()
            )))
        }
    }

    /// load settings from json
    fn from_json(data: &str) -> Result<Self, HttpServerError> {
        serde_json::from_str(data)
            .map_err(|e| HttpServerError::Settings(format!("invalid json: {}", e)))
    }

    /// load settings from toml file
    fn from_toml(data: &str) -> Result<Self, HttpServerError> {
        toml::from_str(data).map_err(HttpServerError::SettingsToml)
    }

    /// Merge settings from other into self
    fn merge(&mut self, other: ServiceSettings) {
        merge!(self, other, address, cache_control, readonly_mode);
        self.tls.merge(other.tls);
        self.cors.merge(other.cors);
        self.log.merge(other.log);
    }

    /// perform additional validation checks on settings.
    /// Several checks have already been done during deserialization.
    /// All errors found are combined into a single error message
    fn validate(&self) -> Result<(), HttpServerError> {
        let mut errors = Vec::new();
        // 1. amke sure address is valid
        if self.address.is_none() {
            errors.push("missing bind address".to_string());
        }
        match (&self.tls.cert_file, &self.tls.priv_key_file) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                errors.push("for tls, both 'cert_file' and 'priv_key_file' must be set".to_string())
            }
            (Some(cert_file), Some(key_file)) => {
                for f in [("cert_file", &cert_file), ("priv_key_file", &key_file)].iter() {
                    let path: &Path = f.1.as_ref();
                    if !path.is_file() {
                        errors.push(format!(
                            "missing tls.{} '{}'{}",
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
        if let Some(ref methods) = self.cors.allowed_methods {
            for m in methods.0.iter() {
                if http::Method::try_from(m.as_str()).is_err() {
                    errors.push(format!("invalid CORS method: '{}'", m));
                }
            }
        }
        if let Some(cache_control) = self.cache_control.as_ref() {
            if http::HeaderValue::from_str(cache_control).is_err() {
                errors.push(format!(
                    "Invalid Cache Control header : '{}'",
                    cache_control
                ));
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

/// Load settings provides a flexible means for loading configuration.
/// Return value is any structure with Deserialize, or for example, HashMap<String,String>
///   config_file: load from file name. Interprets file as json, toml, yaml, based on file extension.
///   config_b64:  base64-encoded json string
///   config_json: raw json string
/// Also accept "address" (a string representing SocketAddr) and "port", a localhost port
/// If more than one key is provided, they are processed in the order above.
///   (later names override earlier names in the list)
///
#[instrument]
pub fn load_settings(values: &HashMap<String, String>) -> Result<ServiceSettings, HttpServerError> {
    trace!("load settings");
    // Allow keys to be UPPERCASE, as an accommodation
    // for the lost souls who prefer ugly all-caps variable names.
    let values: HashMap<String, String> = crate::make_case_insensitive(values).ok_or_else(|| HttpServerError::InvalidParameter(
        "Key collision: httpserver settings (from linkdef.values) has one or more keys that are not unique based on case-insensitivity"
            .to_string(),
    ))?;

    let mut settings = ServiceSettings::default();

    if let Some(fpath) = values.get("config_file") {
        settings.merge(ServiceSettings::from_file(fpath)?);
    }

    if let Some(str) = values.get("config_b64") {
        let bytes = BASE64_STANDARD_NO_PAD
            .decode(str)
            .map_err(|e| HttpServerError::Settings(format!("invalid base64 encoding: {}", e)))?;
        settings.merge(ServiceSettings::from_json(&String::from_utf8_lossy(
            &bytes,
        ))?);
    }

    if let Some(str) = values.get("config_json") {
        settings.merge(ServiceSettings::from_json(str)?);
    }

    // accept address as value parameter
    if let Some(addr) = values.get("address") {
        settings.address = Some(SocketAddr::from_str(addr).map_err(|_| {
            HttpServerError::InvalidParameter(format!("invalid address: {}", addr))
        })?);
    }

    // accept port, for compatibility with previous implementations
    if let Some(addr) = values.get("port") {
        let port = addr
            .parse::<u16>()
            .map_err(|_| HttpServerError::InvalidParameter(format!("Invalid port: {}", addr)))?;
        settings.address = Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            port,
        ));
    }

    // accept cache-control header values
    if let Some(cache_control) = values.get("cache_control") {
        settings.cache_control = Some(cache_control.to_string());
    }

    // accept read only mode flag
    if let Some(readonly_mode) = values.get("readonly_mode") {
        settings.readonly_mode = Some(readonly_mode.to_string().parse().unwrap_or(false));
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

impl Tls {
    fn merge(&mut self, other: Tls) {
        merge!(self, other, cert_file, priv_key_file);
    }
}

impl Tls {
    pub fn is_set(&self) -> bool {
        self.cert_file.is_some() && self.priv_key_file.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cors {
    pub allowed_origins: Option<AllowedOrigins>,

    pub allowed_headers: Option<AllowedHeaders>,

    pub allowed_methods: Option<AllowedMethods>,

    pub exposed_headers: Option<ExposedHeaders>,

    // TODO: allow_credentials?
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

impl Cors {
    fn merge(&mut self, other: Cors) {
        merge!(
            self,
            other,
            allowed_origins,
            allowed_headers,
            allowed_methods,
            exposed_headers,
            max_age_secs
        );
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

/*
/// parse semicolon-delimited origin names
fn parse_allowed_origins(arg: &str) -> Result<AllowedOrigins, std::io::Error> {
    let mut res: Vec<CorsOrigin> = Vec::new();
    for origin_str in arg.split(';') {
        res.push(CorsOrigin::from_str(origin_str)?);
    }
    Ok(AllowedOrigins(res))
}
 */

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
                format!("Invalid uri: {}.\n{}", origin, invalid_uri),
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
                .map(|s| CorsOrigin(s.to_string()))
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
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Disabled,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Log {
    log_level: Option<LogLevel>,
}

impl FromStr for LogLevel {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "disabled" => Ok(Self::Disabled),
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a valid log level", s),
            )),
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Disabled => write!(f, "disabled"),
            Self::Error => write!(f, "error"),
            Self::Warn => write!(f, "warn"),
            Self::Info => write!(f, "info"),
            Self::Debug => write!(f, "debug"),
            Self::Trace => write!(f, "trace"),
        }
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        DEFAULT_LOG_LEVEL
    }
}

impl Log {
    fn merge(&mut self, other: Log) {
        if let Some(level) = other.log_level {
            self.log_level = Some(level);
        }
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
                format!("{} is not a valid http method", s),
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
        assert!(s.address.is_some());

        assert!(s.cors.allowed_methods.is_some());
        assert!(s.cors.allowed_origins.is_some());

        assert!(s.cors.allowed_origins.unwrap().0.is_empty())
    }

    #[test]
    fn settings_toml() {
        let toml = r#"
    [cors]
    allowed_methods = [ "GET" ]
    "#;

        let s = ServiceSettings::from_toml(toml).expect("parse_toml");
        assert_eq!(s.cors.allowed_methods.as_ref().unwrap().0.len(), 1);
        assert_eq!(
            s.cors.allowed_methods.as_ref().unwrap().0.first().unwrap(),
            "GET"
        );
    }

    #[test]
    fn settings_json() {
        let json = r#"{
        "cors": {
            "allowed_headers": [ "X-Cookies" ]
         }
         }"#;

        let s = ServiceSettings::from_json(json).expect("parse_json");
        assert_eq!(s.cors.allowed_headers.as_ref().unwrap().0.len(), 1);
        assert_eq!(
            s.cors.allowed_headers.as_ref().unwrap().0.first().unwrap(),
            "X-Cookies"
        );
    }

    #[test]
    fn origins_deserialize() {
        // test CorsOrigin
        for valid in GOOD_ORIGINS.iter() {
            let o =
                serde_json::from_value::<CorsOrigin>(serde_json::Value::String(valid.to_string()));
            assert!(o.is_ok(), "from_value '{}'", valid);

            // test as_ref()
            assert_eq!(&o.unwrap().0, valid);
        }
    }

    #[test]
    fn origins_from_str() {
        // test CorsOrigin
        for &valid in GOOD_ORIGINS.iter() {
            let o = CorsOrigin::from_str(valid);
            println!("{}: {:?}", valid, o);
            assert!(o.is_ok(), "from_str '{}'", valid);

            // test as_ref()
            assert_eq!(&o.unwrap().0, valid);
        }
    }

    #[test]
    fn origins_negative() {
        for bad in BAD_ORIGINS.iter() {
            let o =
                serde_json::from_value::<CorsOrigin>(serde_json::Value::String(bad.to_string()));
            println!("{}: {:?}", bad, o);
            assert!(o.is_err(), "from_value '{}' (expect err)", bad);

            let o = serde_json::from_str::<CorsOrigin>(bad);
            println!("{}: {:?}", bad, o);
            assert!(o.is_err(), "from_str '{}' (expect err)", bad);
        }
    }
}
