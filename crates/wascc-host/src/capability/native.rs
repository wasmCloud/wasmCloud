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
pub struct NativeCapability {
    pub(crate) plugin: Box<dyn CapabilityProvider>,
    pub(crate) binding_name: String,
    pub(crate) descriptor: CapabilityDescriptor,
    pub(crate) claims: Claims<wascap::jwt::CapabilityProvider>,
    // This field is solely used to keep the FFI library instance allocated for the same
    // lifetime as the boxed plugin
    #[allow(dead_code)]
    library: Option<Library>,
}

impl NativeCapability {
    /// Reads a capability provider from an archive file. The right architecture/OS plugin
    /// library will be chosen from the file, or an error will result if it isn't found.
    pub fn from_archive(
        archive: &ProviderArchive,
        binding_target_name: Option<String>,
    ) -> Result<Self> {
        if archive.claims().is_none() {
            return Err("No claims found in provider archive file".into());
        }

        let target = Host::native_target();

        match archive.target_bytes(&target) {
            Some(bytes) => {
                let path = std::env::temp_dir();
                let path = path.join(archive.claims().unwrap().subject);
                ::std::fs::create_dir_all(&path)?;
                let path = path.join(&target);
                {
                    let mut tf = File::create(&path)?;
                    tf.write_all(&bytes)?;
                }
                type PluginCreate = unsafe fn() -> *mut dyn CapabilityProvider;
                let library = Library::new(&path)?;

                let plugin = unsafe {
                    let constructor: Symbol<PluginCreate> =
                        library.get(b"__capability_provider_create")?;
                    let boxed_raw = constructor();

                    Box::from_raw(boxed_raw)
                };
                let descriptor = get_descriptor(&plugin)?;
                let binding = binding_target_name.unwrap_or("default".to_string());
                info!(
                    "Loaded native capability provider '{}' v{} ({}) for {}/{}",
                    descriptor.name,
                    descriptor.version,
                    descriptor.revision,
                    descriptor.id,
                    binding
                );

                Ok(NativeCapability {
                    plugin,
                    descriptor,
                    claims: archive.claims().unwrap(),
                    binding_name: binding,
                    library: Some(library),
                })
            }
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
        instance: impl CapabilityProvider,
        binding_target_name: Option<String>,
        claims: Claims<wascap::jwt::CapabilityProvider>,
    ) -> Result<Self> {
        let b: Box<dyn CapabilityProvider> = Box::new(instance);
        let descriptor = get_descriptor(&b)?;
        let binding = binding_target_name.unwrap_or("default".to_string());

        info!(
            "Loaded native capability provider library '{}' - {}",
            descriptor.name, &claims.subject
        );
        Ok(NativeCapability {
            descriptor,
            plugin: b,
            claims,
            binding_name: binding,
            library: None,
        })
    }

    /// Returns the capability contract ID of the provider
    pub fn contract_id(&self) -> String {
        self.descriptor.id.to_string()
    }

    /// Returns the unique ID (public key/subject) of the capability provider
    pub fn id(&self) -> String {
        self.claims.subject.to_string()
    }

    /// Returns the human-friendly name of the provider
    pub fn name(&self) -> String {
        self.descriptor.name.to_string()
    }

    /// Returns the full descriptor for the capability provider
    pub fn descriptor(&self) -> &CapabilityDescriptor {
        &self.descriptor
    }
}

fn get_descriptor(plugin: &Box<dyn CapabilityProvider>) -> Result<CapabilityDescriptor> {
    if let Ok(v) = plugin.handle_call(SYSTEM_ACTOR, OP_GET_CAPABILITY_DESCRIPTOR, &[]) {
        match crate::generated::core::deserialize::<CapabilityDescriptor>(&v) {
            Ok(c) => Ok(c),
            Err(e) => Err(format!("Failed to deserialize descriptor: {}", e).into()),
        }
    } else {
        Err("Failed to invoke GetCapabilityDescriptor".into())
    }
}
