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
    /// Enable the wasmcloud:messaging@v3 interface support in the host
    pub(crate) wasmcloud_messaging_v3: bool,
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

    /// Enable the wasmcloud:messaging@v3 interface support in the host
    pub fn enable_wasmcloud_messaging_v3(mut self) -> Self {
        self.wasmcloud_messaging_v3 = true;
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
            wasmcloud_messaging_v3: self.wasmcloud_messaging_v3 || rhs.wasmcloud_messaging_v3,
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
            "wasmcloud-messaging-v3" | "wasmcloud_messaging_v3" => {
                Self::new().enable_wasmcloud_messaging_v3()
            }
            _ => {
                warn!(%s, "unknown feature flag");
                Self::new()
            }
        }
    }
}

/// Convert the host feature flags to the runtime feature flags
impl From<Features> for wasmcloud_runtime::experimental::Features {
    fn from(f: Features) -> wasmcloud_runtime::experimental::Features {
        wasmcloud_runtime::experimental::Features {
            wasmcloud_messaging_v3: f.wasmcloud_messaging_v3,
        }
    }
}
