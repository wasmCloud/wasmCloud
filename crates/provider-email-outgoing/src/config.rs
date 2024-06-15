use serde::Deserialize;

/// Alias for configuration names
pub(crate) type ConfigName = String;

/// Configuration of an outgoing email
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct OutgoingEmailConfig {
    /// Name of the outgoing email config (this is user-providable)
    pub(crate) name: ConfigName,
    /// SMTP send configuration (if present)
    pub(crate) smtp: Option<SmtpConfig>,
}

/// Configuration for accessing an SMTP server
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SmtpConfig {
    /// SMTP URL to use
    ///
    /// ex. 'smtps://smtp.example.com' (SMTP over TLS)
    /// ex. 'smtps://user:password@smtp.example.com' (SMTP over TLS, with auth)
    /// ex. 'smtp://smtp.example.com?tls=required' (SMTP via STARTTLS)
    /// ex. 'smtp://smtp.example.com' (unencrypted SMTP)
    /// ex. 'smtp://127.0.0.1:1025' (unencrypted SMTP)
    ///
    /// see: https://docs.rs/lettre/latest/lettre/transport/smtp/struct.AsyncSmtpTransport.html#method.relay
    ///
    pub(crate) url: String,
    /// Require TLS
    pub(crate) auth: Option<SmtpAuthConfig>,
}

/// Authentication configuration for an SMTP server
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SmtpAuthConfig {
    /// Username to use when accessing the server
    pub(crate) username: String,
    /// Password to use when accessing the server
    pub(crate) password: String,
}
