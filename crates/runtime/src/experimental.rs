use tracing::warn;

/// Feature flags to enable experimental functionality in the runtime. Flags are disabled
/// by default and must be explicitly enabled.
#[derive(Copy, Clone, Debug, Default)]
pub struct Features {
    /// Enable the wasmcloud:messaging@v3 interface support in the runtime
    pub wasmcloud_messaging_v3: bool,
}

impl Features {
    /// Create a new set of feature flags with all features disabled
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the wasmcloud:messaging@v3 interface support in the runtime
    #[must_use]
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
