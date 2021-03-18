use std::{env::temp_dir, path::PathBuf};

use crate::{Host, Result};
use provider_archive::ProviderArchive;
use wascap::jwt::Claims;
use wasmcloud_provider_core::capabilities::CapabilityProvider;

/// Represents a native capability provider compiled as a shared object library.
/// These plugins are OS- and architecture-specific, so they will be `.so` files on Linux, `.dylib`
/// files on macOS, etc.
#[derive(Clone)]
pub struct NativeCapability {
    pub(crate) plugin: Option<Box<dyn CapabilityProvider>>,
    pub(crate) link_name: String,
    pub(crate) claims: Claims<wascap::jwt::CapabilityProvider>,
    pub(crate) native_bytes: Option<Vec<u8>>,
}

impl NativeCapability {
    /// Reads a capability provider from an archive file. The right architecture/OS plugin
    /// library will be chosen from the file, or an error will result if it isn't found.
    pub fn from_archive(
        archive: &ProviderArchive,
        link_target_name: Option<String>,
    ) -> Result<Self> {
        if archive.claims().is_none() {
            return Err("No claims found in provider archive file".into());
        }

        let link = normalize_link_name(link_target_name.unwrap_or_else(|| "default".to_string()));

        let target = Host::native_target();

        match archive.target_bytes(&target) {
            Some(bytes) => Ok(NativeCapability {
                claims: archive.claims().unwrap(),
                link_name: link,
                native_bytes: Some(bytes),
                plugin: None,
            }),
            None => Err(format!(
                "No binary found in archive for target {}",
                Host::native_target()
            )
            .into()),
        }
    }

    /// This function is to be used for _capability embedding_. If you are building a custom
    /// wasmcloud host and have a fixed set of capabilities that you want to always be available
    /// to actors, then you can declare a dependency on the capability provider, enable
    /// the `static_plugin` feature, and provide an instance of that provider. Be sure to check
    /// that the provider supports capability embedding. You must also provide a set of valid
    /// claims that can be generated from a signed JWT
    pub fn from_instance(
        instance: impl CapabilityProvider + 'static,
        link_target_name: Option<String>,
        claims: Claims<wascap::jwt::CapabilityProvider>,
    ) -> Result<Self> {
        let b: Box<dyn CapabilityProvider> = Box::new(instance);
        let link = normalize_link_name(link_target_name.unwrap_or_else(|| "default".to_string()));

        Ok(NativeCapability {
            plugin: Some(b),
            native_bytes: None,
            claims,
            link_name: link,
        })
    }

    /// Returns the unique ID (public key/subject) of the capability provider
    pub fn id(&self) -> String {
        self.claims.subject.to_string()
    }

    pub fn cache_path(&self) -> PathBuf {
        let mut path = temp_dir();
        path.push("wasmcloudcache");
        path.push(&self.claims.subject);
        path.push(format!(
            "{}",
            self.claims.metadata.as_ref().unwrap().rev.unwrap_or(0)
        ));
        path.push(Host::native_target());
        path
    }
}

/// Helper function to unwrap link name. Returns link name if exists and non-empty, "default" otherwise
pub(crate) fn normalize_link_name(link_name: String) -> String {
    if link_name.trim().is_empty() {
        "default".to_string()
    } else {
        link_name
    }
}
