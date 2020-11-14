use crate::{Host, Result, SYSTEM_ACTOR};
use libloading::Library;
use libloading::Symbol;
use provider_archive::ProviderArchive;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use wascap::jwt::Claims;
use wascc_codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, OP_GET_CAPABILITY_DESCRIPTOR,
};

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
        let link = link_target_name.unwrap_or("default".to_string());

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
    /// waSCC host and have a fixed set of capabilities that you want to always be available
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
        let link = link_target_name.unwrap_or("default".to_string());

        Ok(NativeCapability {
            plugin: Some(b),
            native_bytes: None,
            claims: claims.clone(),
            link_name: link,
        })
    }

    /// Returns the unique ID (public key/subject) of the capability provider
    pub fn id(&self) -> String {
        self.claims.subject.to_string()
    }
}
