//! Core reusable functionality related to [WebAssembly Interface types ("WIT")](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)

use std::collections::HashMap;

use anyhow::Context;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::{WitFunction, WitInterface, WitNamespace, WitPackage};

// I don't know if these would be generated or if we'd just include them in the library and then use them in the generated code, but they work around the lack of a map type in wit

/// Representation of maps (AKA associative arrays) that are usable from WIT
///
/// This representation is required because WIT does not natively
/// have support for a map type, so we must use a list of tuples
pub type WitMap<T> = Vec<(String, T)>;

pub(crate) fn serialize_wit_map<S: Serializer, T>(
    map: &WitMap<T>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
{
    let mut seq = serializer.serialize_map(Some(map.len()))?;
    for (key, val) in map {
        seq.serialize_entry(key, val)?;
    }
    seq.end()
}

pub(crate) fn deserialize_wit_map<'de, D: serde::Deserializer<'de>, T>(
    deserializer: D,
) -> Result<WitMap<T>, D::Error>
where
    T: Deserialize<'de>,
{
    let values = HashMap::<String, T>::deserialize(deserializer)?;
    Ok(values.into_iter().collect())
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
/// Call target identifier, which is equivalent to a WIT specification, which
/// can identify an interface being called and optionally a specific function on that interface.
pub struct CallTargetInterface {
    /// WIT namespace (ex. `wasi` in `wasi:keyvalue/readwrite.get`)
    pub namespace: String,
    /// WIT package name (ex. `keyvalue` in `wasi:keyvalue/readwrite.get`)
    pub package: String,
    /// WIT interface (ex. `readwrite` in `wasi:keyvalue/readwrite.get`)
    pub interface: String,
}

impl CallTargetInterface {
    /// Returns the 3-tuple of (namespace, package, interface) for this interface
    #[must_use]
    pub fn as_parts(&self) -> (&str, &str, &str) {
        (&self.namespace, &self.package, &self.interface)
    }

    /// Build a [`TargetInterface`] from constituent parts
    #[must_use]
    pub fn from_parts((ns, pkg, iface): (&str, &str, &str)) -> Self {
        Self {
            namespace: ns.into(),
            package: pkg.into(),
            interface: iface.into(),
        }
    }

    /// Build a target interface from a given operation
    pub fn from_operation(operation: impl AsRef<str>) -> anyhow::Result<Self> {
        let operation = operation.as_ref();
        let (wit_ns, wit_pkg, wit_iface, _) = parse_wit_meta_from_operation(operation)?;
        Ok(CallTargetInterface::from_parts((
            &wit_ns, &wit_pkg, &wit_iface,
        )))
    }
}

/// Parse a sufficiently specified WIT operation/method into constituent parts.
///
///
/// # Errors
///
/// Returns `Err` if the operation is not of the form "<package>:<ns>/<interface>.<function>"
///
/// # Example
///
/// ```
/// let (wit_ns, wit_pkg, wit_iface, wit_fn) = parse_wit_meta_from_operation(("wasmcloud:bus/guest-config"));
/// #assert_eq!(wit_ns, "wasmcloud")
/// #assert_eq!(wit_pkg, "bus")
/// #assert_eq!(wit_iface, "iface")
/// #assert_eq!(wit_fn, None)
/// let (wit_ns, wit_pkg, wit_iface, wit_fn) = parse_wit_meta_from_operation(("wasmcloud:bus/guest-config.get"));
/// #assert_eq!(wit_ns, "wasmcloud")
/// #assert_eq!(wit_pkg, "bus")
/// #assert_eq!(wit_iface, "iface")
/// #assert_eq!(wit_fn, Some("get"))
/// ```
pub fn parse_wit_meta_from_operation(
    operation: impl AsRef<str>,
) -> anyhow::Result<(WitNamespace, WitPackage, WitInterface, Option<WitFunction>)> {
    let operation = operation.as_ref();
    let (ns_and_pkg, interface_and_func) = operation
        .rsplit_once('/')
        .context("failed to parse operation")?;
    let (wit_iface, wit_fn) = interface_and_func
        .split_once('.')
        .context("interface and function should be specified")?;
    let (wit_ns, wit_pkg) = ns_and_pkg
        .rsplit_once(':')
        .context("failed to parse operation for WIT ns/pkg")?;
    Ok((
        wit_ns.into(),
        wit_pkg.into(),
        wit_iface.into(),
        if wit_fn.is_empty() {
            None
        } else {
            Some(wit_fn.into())
        },
    ))
}
