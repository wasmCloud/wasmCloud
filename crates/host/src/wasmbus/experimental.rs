use tracing::warn;

/// Feature flags to enable experimental functionality in the host. Flags are disabled
/// by default and must be explicitly enabled.
#[derive(Copy, Clone, Debug, Default)]
pub struct Features {
    /// Enable the built-in HTTP server capability provider
    /// that can be started with the reference wasmcloud+builtin://http-server
    pub(crate) builtin_http_server: bool,
    /// Enable the built-in NATS Messaging capability provider
    /// that can be started with the reference wasmcloud+builtin://messaging-nats
    pub(crate) builtin_messaging_nats: bool,
}

impl Features {
    /// Create a new set of feature flags with all features disabled
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the built-in HTTP server capability provider
    pub fn enable_builtin_http_server(mut self) -> Self {
        self.builtin_http_server = true;
        self
    }

    /// Enable the built-in NATS messaging capability provider
    pub fn enable_builtin_messaging_nats(mut self) -> Self {
        self.builtin_messaging_nats = true;
        self
    }
}

/// This enables unioning feature flags together
impl std::ops::BitOr for Features {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            builtin_http_server: self.builtin_http_server || rhs.builtin_http_server,
            builtin_messaging_nats: self.builtin_messaging_nats || rhs.builtin_messaging_nats,
        }
    }
}

/// Allow for summing over a collection of feature flags
impl std::iter::Sum for Features {
    fn sum<I: Iterator<Item = Self>>(mut iter: I) -> Self {
        // Grab the first set of flags, fall back on defaults (all disabled)
        let first = iter.next().unwrap_or_default();
        iter.fold(first, |a, b| a | b)
    }
}

/// Parse a feature flag from a string, enabling the feature if the string matches
impl From<&str> for Features {
    fn from(s: &str) -> Self {
        match &*s.to_ascii_lowercase() {
            "builtin-http-server" | "builtin_http_server" => {
                Self::new().enable_builtin_http_server()
            }
            "builtin-messaging-nats" | "builtin_messaging_nats" => {
                Self::new().enable_builtin_messaging_nats()
            }
            _ => {
                warn!(%s, "unknown feature flag");
                Self::new()
            }
        }
    }
}
