//! Reusable functionality related to [WebAssembly Interface types ("WIT")][wit]
//!
//! [wit]: <https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md>

use std::collections::HashMap;

use anyhow::{bail, Context as _, Result};
use semver::Version;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::{WitFunction, WitInterface, WitNamespace, WitPackage};

/// Representation of maps (AKA associative arrays) that are usable from WIT
///
/// This representation is required because WIT does not natively
/// have support for a map type, so we must use a list of tuples
pub type WitMap<T> = Vec<(String, T)>;

pub(crate) fn serialize_wit_map<S: Serializer, T>(
    map: &WitMap<T>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
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
) -> std::result::Result<WitMap<T>, D::Error>
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

    /// Returns the fully qualified WIT interface in the form `namespace:package/interface`
    pub fn as_instance(&self) -> String {
        format!("{}:{}/{}", self.namespace, self.package, self.interface)
    }

    /// Build a [`CallTargetInterface`] from constituent parts
    #[must_use]
    pub fn from_parts((ns, pkg, iface): (&str, &str, &str)) -> Self {
        Self {
            namespace: ns.into(),
            package: pkg.into(),
            interface: iface.into(),
        }
    }

    /// Build a target interface from a given operation
    pub fn from_operation(operation: impl AsRef<str>) -> Result<Self> {
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
/// Returns `Err` if the operation is not of the form "&lt;package&gt;:&lt;ns&gt;/&lt;interface&gt;.&lt;function&gt;"
///
/// # Example
///
/// ```
/// # use wasmcloud_core::parse_wit_meta_from_operation;
/// let (wit_ns, wit_pkg, wit_iface, wit_fn) = parse_wit_meta_from_operation("wasmcloud:bus/guest-config").unwrap();
/// # assert_eq!(wit_ns, "wasmcloud".to_string());
/// # assert_eq!(wit_pkg, "bus".to_string());
/// # assert_eq!(wit_iface, "guest-config".to_string());
/// # assert_eq!(wit_fn, None);
/// let (wit_ns, wit_pkg, wit_iface, wit_fn) = parse_wit_meta_from_operation("wasmcloud:bus/guest-config.get").unwrap();
/// # assert_eq!(wit_ns, "wasmcloud".to_string());
/// # assert_eq!(wit_pkg, "bus".to_string());
/// # assert_eq!(wit_iface, "guest-config".to_string());
/// # assert_eq!(wit_fn, Some("get".to_string()));
/// ```
pub fn parse_wit_meta_from_operation(
    operation: impl AsRef<str>,
) -> Result<(WitNamespace, WitPackage, WitInterface, Option<WitFunction>)> {
    let operation = operation.as_ref();
    let (ns_and_pkg, interface_and_func) = operation
        .rsplit_once('/')
        .context("failed to parse operation")?;
    let (wit_ns, wit_pkg) = ns_and_pkg
        .rsplit_once(':')
        .context("failed to parse operation for WIT ns/pkg")?;
    let (wit_iface, wit_fn) = match interface_and_func.split_once('.') {
        Some((iface, func)) => (iface, Some(func.to_string())),
        None => (interface_and_func, None),
    };
    Ok((wit_ns.into(), wit_pkg.into(), wit_iface.into(), wit_fn))
}

type WitInformationTuple = (
    WitNamespace,
    Vec<WitPackage>,
    Option<Vec<WitInterface>>,
    Option<WitFunction>,
    Option<Version>,
);

/// Parse a WIT package name into constituent parts.
///
/// This function is `parse_wit_meta_from_operation` but differs
/// in that it allows more portions to be missing, and handles more use cases,
/// like operations on resources
///
/// This formulation should *also* support future nested package/interface features in the WIT spec.
///
/// # Errors
///
/// Returns `Err` if the operation is not of the form "&lt;package&gt;:&lt;ns&gt;/&lt;interface&gt;.&lt;function&gt;"
///
/// # Example
///
/// ```
/// # use semver::Version;
/// # use wasmcloud_core::parse_wit_package_name;
/// let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasi:http").unwrap();
/// # assert_eq!(ns, "wasi".to_string());
/// # assert_eq!(packages, vec!["http".to_string()]);
/// # assert_eq!(interfaces, None);
/// # assert_eq!(func, None);
/// # assert_eq!(version, None);
/// let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasi:http@0.2.2").unwrap();
/// # assert_eq!(ns, "wasi".to_string());
/// # assert_eq!(packages, vec!["http".to_string()]);
/// # assert_eq!(interfaces, None);
/// # assert_eq!(func, None);
/// # assert_eq!(version, Version::parse("0.2.2").ok());
/// let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasmcloud:bus/guest-config").unwrap();
/// # assert_eq!(ns, "wasmcloud");
/// # assert_eq!(packages, vec!["bus".to_string()]);
/// # assert_eq!(interfaces, Some(vec!["guest-config".to_string()]));
/// # assert_eq!(func, None);
/// # assert_eq!(version, None);
/// let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasmcloud:bus/guest-config.get").unwrap();
/// # assert_eq!(ns, "wasmcloud");
/// # assert_eq!(packages, vec!["bus".to_string()]);
/// # assert_eq!(interfaces, Some(vec!["guest-config".to_string()]));
/// # assert_eq!(func, Some("get".to_string()));
/// # assert_eq!(version, None);
/// let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasi:http/incoming-handler@0.2.0").unwrap();
/// # assert_eq!(ns, "wasi".to_string());
/// # assert_eq!(packages, vec!["http".to_string()]);
/// # assert_eq!(interfaces, Some(vec!["incoming-handler".to_string()]));
/// # assert_eq!(func, None);
/// # assert_eq!(version, Version::parse("0.2.0").ok());
/// let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasi:keyvalue/atomics.increment@0.2.0-draft").unwrap();
/// # assert_eq!(ns, "wasi".to_string());
/// # assert_eq!(packages, vec!["keyvalue".to_string()]);
/// # assert_eq!(interfaces, Some(vec!["atomics".to_string()]));
/// # assert_eq!(func, Some("increment".to_string()));
/// # assert_eq!(version, Version::parse("0.2.0-draft").ok());
/// ```
pub fn parse_wit_package_name(p: impl AsRef<str>) -> Result<WitInformationTuple> {
    let p = p.as_ref();
    // If there's a version, we can strip it off first and parse it
    let (rest, version) = match p.rsplit_once('@') {
        Some((rest, version)) => (
            rest,
            Some(
                Version::parse(version)
                    .map_err(|e| anyhow::anyhow!(e))
                    .with_context(|| {
                        format!("failed to parse version from wit package name [{p}]")
                    })?,
            ),
        ),
        None => (p, None),
    };

    // Read to the first '/' which should mark the first package
    let (ns_and_pkg, interface_and_func) = match rest.rsplit_once('/') {
        Some((ns_and_pkg, interface_and_func)) => (ns_and_pkg, Some(interface_and_func)),
        None => (rest, None),
    };

    // Read all packages
    let ns_pkg_split = ns_and_pkg.split(':').collect::<Vec<&str>>();
    let (ns, packages) = match ns_pkg_split[..] {
        [] => bail!("invalid package name, missing namespace & package"),
        [_] => bail!("invalid package name, invalid package"),
        [ns, ref packages @ ..] => (ns, packages),
    };

    // Read all interfaces
    let (mut interfaces, iface_with_fn) = match interface_and_func
        .unwrap_or_default()
        .split('/')
        .filter(|v| !v.is_empty())
        .collect::<Vec<&str>>()[..]
    {
        [] => (None, None),
        [iface] => (Some(vec![]), Some(iface)),
        [iface, f] => (Some(vec![iface]), Some(f)),
        [ref ifaces @ .., f] => (Some(Vec::from(ifaces)), Some(f)),
    };

    let func = match iface_with_fn {
        Some(iface_with_fn) => match iface_with_fn.split_once('.') {
            Some((iface, f)) => {
                if let Some(ref mut interfaces) = interfaces {
                    interfaces.push(iface);
                };
                Some(f)
            }
            None => {
                if let Some(ref mut interfaces) = interfaces {
                    interfaces.push(iface_with_fn);
                };
                None
            }
        },
        None => None,
    };

    Ok((
        ns.into(),
        packages
            .iter()
            .map(|v| String::from(*v))
            .collect::<Vec<_>>(),
        interfaces.map(|v| v.into_iter().map(String::from).collect::<Vec<_>>()),
        func.map(String::from),
        version,
    ))
}

// TODO(joonas): Remove these once doctests are run as part of CI.
#[cfg(test)]
mod test {
    use semver::Version;

    use super::parse_wit_package_name;
    #[test]
    fn test_parse_wit_package_name() {
        let (ns, packages, interfaces, func, version) =
            parse_wit_package_name("wasi:http").expect("should have parsed'wasi:http'");
        assert_eq!(ns, "wasi".to_string());
        assert_eq!(packages, vec!["http".to_string()]);
        assert_eq!(interfaces, None);
        assert_eq!(func, None);
        assert_eq!(version, None);

        let (ns, packages, interfaces, func, version) = parse_wit_package_name("wasi:http@0.2.2")
            .expect("should have parsed 'wasi:http@0.2.2'");
        assert_eq!(ns, "wasi".to_string());
        assert_eq!(packages, vec!["http".to_string()]);
        assert_eq!(interfaces, None);
        assert_eq!(func, None);
        assert_eq!(version, Version::parse("0.2.2").ok());

        let (ns, packages, interfaces, func, version) =
            parse_wit_package_name("wasmcloud:bus/guest-config")
                .expect("should have parsed 'wasmcloud:bus/guest-config'");
        assert_eq!(ns, "wasmcloud");
        assert_eq!(packages, vec!["bus".to_string()]);
        assert_eq!(interfaces, Some(vec!["guest-config".to_string()]));
        assert_eq!(func, None);
        assert_eq!(version, None);

        let (ns, packages, interfaces, func, version) =
            parse_wit_package_name("wasmcloud:bus/guest-config.get")
                .expect("should have parsed 'wasmcloud:bus/guest-config.get'");
        assert_eq!(ns, "wasmcloud");
        assert_eq!(packages, vec!["bus".to_string()]);
        assert_eq!(interfaces, Some(vec!["guest-config".to_string()]));
        assert_eq!(func, Some("get".to_string()));
        assert_eq!(version, None);

        let (ns, packages, interfaces, func, version) =
            parse_wit_package_name("wasi:http/incoming-handler@0.2.0")
                .expect("should have parsed 'wasi:http/incoming-handler@0.2.0'");
        assert_eq!(ns, "wasi".to_string());
        assert_eq!(packages, vec!["http".to_string()]);
        assert_eq!(interfaces, Some(vec!["incoming-handler".to_string()]));
        assert_eq!(func, None);
        assert_eq!(version, Version::parse("0.2.0").ok());

        let (ns, packages, interfaces, func, version) =
            parse_wit_package_name("wasi:keyvalue/atomics.increment@0.2.0-draft")
                .expect("should have parsed 'wasi:keyvalue/atomics.increment@0.2.0-draft'");
        assert_eq!(ns, "wasi".to_string());
        assert_eq!(packages, vec!["keyvalue".to_string()]);
        assert_eq!(interfaces, Some(vec!["atomics".to_string()]));
        assert_eq!(func, Some("increment".to_string()));
        assert_eq!(version, Version::parse("0.2.0-draft").ok());
    }
}
