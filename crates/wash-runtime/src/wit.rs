//! WebAssembly Interface Types (WIT) support for wasmcloud.
//!
//! This module provides types and utilities for working with WIT interface
//! specifications. WIT is used to describe the capabilities that components
//! require and that plugins provide.
//!
//! # Key Types
//!
//! - [`WitWorld`] - A collection of imports and exports representing a WIT world
//! - [`WitInterface`] - A specific interface specification with namespace, package, and version
//!
//! # Interface Matching
//!
//! The [`WitInterface::contains`] method is used to determine if one interface
//! specification can satisfy another. This is crucial for matching component
//! requirements with plugin capabilities.

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    hash::DefaultHasher,
};

use serde::{Deserialize, Serialize};

/// A collection of WIT interfaces representing a world definition.
///
/// A WIT world describes the imports and exports that a component or
/// plugin provides. This is used for capability matching between
/// workloads and plugins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WitWorld {
    /// The interfaces that this world imports (requires from the host)
    pub imports: HashSet<WitInterface>,
    /// The interfaces that this world exports (provides to the host)
    pub exports: HashSet<WitInterface>,
}

impl WitWorld {
    /// This function checks if the world includes a specific interface. This is
    /// slightly different than checking directly against the imports or exports
    /// because it takes into account the possibility of interface nesting and
    /// versioning. See [`WitInterface::contains`] for more details.
    pub fn includes(&self, interface: &WitInterface) -> bool {
        self.imports.iter().any(|i| i.contains(interface))
            || self.exports.iter().any(|e| e.contains(interface))
    }

    /// This function checks if the world includes a specific interface. This is
    /// different than [`WitWorld::includes`] because it considers that in one
    /// [`WitInterface`] there may be both imports and exports.
    pub fn includes_bidirectional(&self, interface: &WitInterface) -> bool {
        let import_match = self.imports.iter().find(|i| {
            if let Some(v) = &interface.version
                && let Some(ov) = &i.version
                && v != ov
            {
                return false;
            }
            i.namespace == interface.namespace && i.package == interface.package
        });

        let export_match = self.exports.iter().find(|e| {
            // If both interfaces specify a version, they must match
            if let Some(v) = &interface.version
                && let Some(ov) = &e.version
                && v != ov
            {
                return false;
            }
            e.namespace == interface.namespace && e.package == interface.package
        });

        // Ensure the interfaces are covered by either the import or export match
        for i in &interface.interfaces {
            // Interface satisfied by import
            if let Some(im) = &import_match
                && im.interfaces.contains(i)
            {
                continue;
            }
            // Interface satisfied by export
            if let Some(em) = &export_match
                && em.interfaces.contains(i)
            {
                continue;
            }

            return false;
        }

        true
    }

    /// Checks if a guest world (imports) can be satisfied by a host world (exports).
    ///
    /// A host world satisfies a guest world if all interfaces required by the guest
    /// are provided by the host, considering interface containment (subset/superset)
    /// and ignoring any additional interfaces or versions in the host.
    ///
    /// # Arguments
    /// * `guest` - The guest world to check
    ///
    /// # Returns
    /// `true` if the host world (self) provides all interfaces required by the guest world
    /// through direct exports or through superset relationships.
    pub fn satisfies(&self, guest: &WitWorld) -> bool {
        // For each interface that the guest imports, find a matching export in the host
        for required in &guest.imports {
            // Check if there's a direct match or a superset match
            let matched: Vec<_> = self
                .exports
                .iter()
                .filter(|provided| provided.contains(required))
                .collect();

            // If no matches found, guest cannot be satisfied
            if matched.is_empty() {
                return false;
            }

            // If there's more than one match, we need to ensure it's not a version conflict
            if matched.len() > 1 {
                // All matched exports must have the same version (if versioned)
                let versions: HashSet<_> =
                    matched.iter().filter_map(|m| m.version.as_ref()).collect();
                if versions.len() > 1 {
                    return false; // Conflicting versions
                }
            }
        }

        true
    }
}

/// Represents a WIT interface specification with namespace, package, and optional version.
///
/// A `WitInterface` identifies a specific set of interfaces from a WIT package.
/// It follows the format: `namespace:package/interface1,interface2@version`
/// where interfaces and version are optional.
///
/// # Examples
/// - `wasi:http` - Just namespace and package
/// - `wasi:http/incoming-handler` - With a single interface
/// - `wasi:http/incoming-handler,outgoing-handler@0.2.0` - Multiple interfaces with version
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WitInterface {
    /// The namespace of the interface (e.g., "wasi")
    pub namespace: String,
    /// The package name (e.g., "http", "blobstore")
    pub package: String,
    /// The specific interfaces within the package (e.g., "incoming-handler", "types")
    pub interfaces: HashSet<String>,
    // TODO: This is a nice way to represent a version, but it doesn't account for
    // compatible versions. We should revisit this and implement https://docs.rs/semver/1.0.27/semver/struct.VersionReq.html
    /// Optional semantic version for the interface
    pub version: Option<semver::Version>,
    /// Additional configuration parameters for this interface
    pub config: HashMap<String, String>,
    /// Optional name identifying this specific instance when multiple entries
    /// of the same namespace:package exist. Used as the routing key in
    /// multiplexing plugins (the `identifier` in store::open, etc.).
    pub name: Option<String>,
}

impl WitInterface {
    /// Returns the instance name of this WitInterface, aka the namespace:package@version
    /// identifier without the interfaces or config. When a name is present, it is
    /// included as namespace:package/name@version.
    pub fn instance(&self) -> String {
        let base = match &self.name {
            Some(name) => format!("{}:{}/{name}", self.namespace, self.package),
            None => format!("{}:{}", self.namespace, self.package),
        };
        if let Some(v) = &self.version {
            format!("{base}@{v}")
        } else {
            base
        }
    }

    /// Merges another WitInterface into this one, returning a boolean
    /// indicating whether the merge was successful (aka if the [`WitInterface::instance`]s matched).
    pub fn merge(&mut self, other: &WitInterface) -> bool {
        if self.instance() != other.instance() {
            return false;
        }

        self.interfaces.extend(other.interfaces.clone());
        self.config.extend(other.config.clone());
        true
    }

    /// Checks if this interface contains (is a superset of) another interface.
    ///
    /// This method is used to determine if a plugin or component that provides
    /// this interface can satisfy a requirement for the other interface.
    ///
    /// # Arguments
    /// * `other` - The interface to check against
    ///
    /// # Returns
    /// `true` if:
    /// - The namespace and package match exactly
    /// - If this interface has a version, it must match the other's version
    /// - The other's interfaces are a subset of this interface's interfaces
    pub fn contains(&self, other: &WitInterface) -> bool {
        // Namespace and package must match
        if self.namespace != other.namespace || self.package != other.package {
            return false;
        }

        // If both interfaces specify a version, they must match
        if let Some(v) = &self.version
            && let Some(ov) = &other.version
            && v != ov
        {
            return false;
        }

        // If both interfaces specify a name, they must match
        if let Some(n) = &self.name
            && let Some(on) = &other.name
            && n != on
        {
            return false;
        }

        self.interfaces.is_superset(&other.interfaces)
    }
}

impl Display for WitInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.namespace, self.package)?;
        if let Some(name) = &self.name {
            write!(f, " [{}]", name)?;
        }
        if !self.interfaces.is_empty() {
            write!(f, "/")?;
            let interfaces: Vec<_> = self.interfaces.clone().into_iter().collect();
            write!(f, "{}", interfaces.join(","))?;
        }
        if let Some(v) = &self.version {
            write!(f, "@{}", v)?;
        }
        Ok(())
    }
}

impl std::hash::Hash for WitInterface {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.namespace.hash(state);
        self.package.hash(state);
        self.name.hash(state);
        // HashSet and HashMap have non-deterministic iteration order,
        // so we XOR individual hashes to produce an order-independent result.
        let mut interfaces_hash = 0u64;
        for iface in &self.interfaces {
            let mut h = DefaultHasher::new();
            iface.hash(&mut h);
            interfaces_hash ^= std::hash::Hasher::finish(&h);
        }
        interfaces_hash.hash(state);
        self.version.hash(state);
        let mut config_hash = 0u64;
        for (k, v) in &self.config {
            let mut h = DefaultHasher::new();
            k.hash(&mut h);
            v.hash(&mut h);
            config_hash ^= std::hash::Hasher::finish(&h);
        }
        config_hash.hash(state);
    }
}

impl From<&str> for WitInterface {
    fn from(s: &str) -> Self {
        // Expected format: namespace:package/interface@version
        // Also supports for convenience: namespace:package/interface,interface2,interface3@version
        // interface and version are optional

        let (main, version) = match s.split_once('@') {
            Some((m, v)) => (m, Some(v)),
            None => (s, None),
        };
        let (namespace_package, interface) = match main.split_once('/') {
            Some((np, iface)) => (np, Some(iface)),
            None => (main, None),
        };
        let (namespace, package) = match namespace_package.split_once(':') {
            Some((ns, pkg)) => (ns, pkg),
            None => ("", namespace_package),
        };
        let interfaces = match interface {
            Some(iface) => iface
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            None => HashSet::new(),
        };
        let version = version.and_then(|v| semver::Version::parse(v).ok());

        WitInterface {
            namespace: namespace.to_string(),
            package: package.to_string(),
            interfaces,
            version,
            config: HashMap::new(),
            name: None,
        }
    }
}

impl From<String> for WitInterface {
    fn from(s: String) -> Self {
        WitInterface::from(s.as_str())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn create_interface(namespace: &str, package: &str, interfaces: &[&str]) -> WitInterface {
        WitInterface {
            namespace: namespace.to_string(),
            package: package.to_string(),
            interfaces: interfaces.iter().map(|s| s.to_string()).collect(),
            version: None,
            config: HashMap::new(),
            name: None,
        }
    }

    fn create_interface_with_version(
        namespace: &str,
        package: &str,
        interfaces: &[&str],
        version: &str,
    ) -> WitInterface {
        WitInterface {
            namespace: namespace.to_string(),
            package: package.to_string(),
            interfaces: interfaces.iter().map(|s| s.to_string()).collect(),
            version: Some(semver::Version::parse(version).unwrap()),
            config: HashMap::new(),
            name: None,
        }
    }

    #[test]
    fn test_contains_basic() {
        let interface_a = create_interface("wasi", "logging", &["log", "error", "debug"]);
        let interface_b = create_interface("wasi", "logging", &["log", "error"]);
        let interface_c = create_interface("wasi", "logging", &["log", "trace"]);

        // interface_a contains interface_b (superset)
        assert!(interface_a.contains(&interface_b));
        // interface_b does not contain interface_a (not a superset)
        assert!(!interface_b.contains(&interface_a));
        // interface_a does not contain interface_c (not a superset - missing "trace")
        assert!(!interface_a.contains(&interface_c));
        // An interface contains itself
        assert!(interface_a.contains(&interface_a));
    }

    #[test]
    fn test_contains_namespace_and_package_matching() {
        // Different namespaces should not match
        let wit1 = WitInterface::from("wasi:blobstore");
        let wit2 = WitInterface::from("custom:blobstore");
        assert!(!wit1.contains(&wit2));

        // Different packages should not match
        let wit3 = WitInterface::from("wasi:blobstore");
        let wit4 = WitInterface::from("wasi:keyvalue");
        assert!(!wit3.contains(&wit4));

        // Same namespace and package should match
        let wit5 = WitInterface::from("wasi:blobstore");
        let wit6 = WitInterface::from("wasi:blobstore");
        assert!(wit5.contains(&wit6));

        // Empty namespace cases
        let wit7 = WitInterface::from("blobstore/types");
        let wit8 = WitInterface::from("blobstore/types");
        assert!(wit7.contains(&wit8));

        // Mixed empty namespace should not match
        let wit9 = WitInterface::from("wasi:blobstore/types");
        let wit10 = WitInterface::from("blobstore/types");
        assert!(!wit9.contains(&wit10));
    }

    #[test]
    fn test_contains_interface_subsets() {
        // Subset relationship
        let wit1 = WitInterface::from("wasi:blobstore/types,container,blobstore");
        let wit2 = WitInterface::from("wasi:blobstore/types,container");
        assert!(wit1.contains(&wit2));

        // Not a subset - wit2 has interfaces wit1 doesn't have
        let wit3 = WitInterface::from("wasi:blobstore/types");
        let wit4 = WitInterface::from("wasi:blobstore/types,container");
        assert!(!wit3.contains(&wit4));

        // Empty set is a subset of any set
        let wit5 = WitInterface::from("wasi:blobstore/types,container");
        let wit6 = WitInterface::from("wasi:blobstore");
        assert!(wit5.contains(&wit6));

        // Overlapping but not subset
        let wit7 = WitInterface::from("wasi:cli/stdin,stdout");
        let wit8 = WitInterface::from("wasi:cli/stdout,stderr");
        assert!(!wit7.contains(&wit8));

        // Single interface vs multiple
        let wit9 = WitInterface::from("wasi:cli/environment,exit,stdin,stdout");
        let wit10 = WitInterface::from("wasi:cli/environment");
        assert!(wit9.contains(&wit10));
    }

    #[test]
    fn test_contains_version_handling() {
        // Same versions should match
        let wit1 = WitInterface::from("wasi:blobstore/types@0.2.0");
        let wit2 = WitInterface::from("wasi:blobstore/types@0.2.0");
        assert!(wit1.contains(&wit2));

        // Different versions should not match
        let wit3 = WitInterface::from("wasi:blobstore/types@0.2.0");
        let wit4 = WitInterface::from("wasi:blobstore/types@0.3.0");
        assert!(!wit3.contains(&wit4));

        // wit1 has version, wit2 doesn't - still matches (wit2 is less restrictive)
        let wit5 = WitInterface::from("wasi:blobstore/types@0.2.0");
        let wit6 = WitInterface::from("wasi:blobstore/types");
        assert!(wit5.contains(&wit6));

        // wit1 has no version requirement, wit2 can have any version
        let wit7 = WitInterface::from("wasi:blobstore/types");
        let wit8 = WitInterface::from("wasi:blobstore/types@0.2.0");
        assert!(wit7.contains(&wit8));

        // Complex scenario with version
        let wit9 = WitInterface::from("wasi:http/types,incoming-handler,outgoing-handler@0.2.0");
        let wit10 = WitInterface::from("wasi:http/types,incoming-handler@0.2.0");
        assert!(wit9.contains(&wit10));
    }

    #[test]
    fn test_contains_with_version() {
        let interface_a = create_interface_with_version("wasi", "http", &["handler"], "0.2.0");
        let interface_b = create_interface_with_version("wasi", "http", &["handler"], "0.2.0");
        let interface_c = create_interface_with_version("wasi", "http", &["handler"], "0.3.0");

        // Same versions should match
        assert!(interface_a.contains(&interface_b));
        // Different versions should not match
        assert!(!interface_a.contains(&interface_c));
    }

    #[test]
    fn test_contains_config_ignored() {
        // Config doesn't affect contains logic, only namespace, package, interfaces, and version matter
        let mut wit1 = WitInterface::from("wasi:blobstore/types");
        wit1.config.insert("key".to_string(), "value1".to_string());

        let mut wit2 = WitInterface::from("wasi:blobstore/types");
        wit2.config.insert("key".to_string(), "value2".to_string());

        // Config differences are ignored
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_world_includes() {
        let required_interface = create_interface("wasi", "keyvalue", &["get"]);
        let broader_interface = create_interface("wasi", "keyvalue", &["get", "set"]);
        let different_interface = create_interface("wasi", "logging", &["log"]);

        // World that imports the exact interface
        let world1 = WitWorld {
            imports: [required_interface.clone()].iter().cloned().collect(),
            exports: HashSet::new(),
        };
        assert!(world1.includes(&required_interface));

        // World that imports a broader interface
        let world2 = WitWorld {
            imports: [broader_interface.clone()].iter().cloned().collect(),
            exports: HashSet::new(),
        };
        assert!(world2.includes(&required_interface));
        assert!(!world1.includes(&broader_interface));

        // World that exports the interface
        let world3 = WitWorld {
            imports: HashSet::new(),
            exports: [broader_interface.clone()].iter().cloned().collect(),
        };
        assert!(world3.includes(&required_interface));

        // World with a non-matching interface
        let world4 = WitWorld {
            imports: [different_interface].iter().cloned().collect(),
            exports: HashSet::new(),
        };
        assert!(!world4.includes(&required_interface));
    }

    #[test]
    fn test_world_satisfies() {
        // Guest requires 'logging' and 'keyvalue-read'
        let guest_world = WitWorld {
            imports: [
                create_interface("wasi", "logging", &["log"]),
                create_interface("wasi", "keyvalue", &["get", "exists"]),
            ]
            .iter()
            .cloned()
            .collect(),
            exports: HashSet::new(),
        };

        // Host that provides exactly what's needed
        let host_world_exact = WitWorld {
            imports: HashSet::new(),
            exports: [
                create_interface("wasi", "logging", &["log"]),
                create_interface("wasi", "keyvalue", &["get", "exists"]),
            ]
            .iter()
            .cloned()
            .collect(),
        };
        assert!(host_world_exact.satisfies(&guest_world));

        // Host that provides superset interfaces
        let host_world_superset = WitWorld {
            imports: HashSet::new(),
            exports: [
                create_interface("wasi", "logging", &["log", "error"]),
                create_interface("wasi", "keyvalue", &["get", "exists", "set", "delete"]),
            ]
            .iter()
            .cloned()
            .collect(),
        };
        assert!(host_world_superset.satisfies(&guest_world));

        // Host that is missing one of the required interfaces
        let host_world_missing = WitWorld {
            imports: HashSet::new(),
            exports: [create_interface("wasi", "logging", &["log"])]
                .iter()
                .cloned()
                .collect(),
        };
        assert!(!host_world_missing.satisfies(&guest_world));

        // Host that provides a subset of a required interface (not enough)
        let host_world_subset = WitWorld {
            imports: HashSet::new(),
            exports: [
                create_interface("wasi", "logging", &["log"]),
                create_interface("wasi", "keyvalue", &["get"]), // Missing "exists"
            ]
            .iter()
            .cloned()
            .collect(),
        };
        assert!(!host_world_subset.satisfies(&guest_world));
    }

    #[test]
    fn test_parse_basic_formats() {
        // Basic namespace:package
        let wit1 = WitInterface::from("wasi:blobstore");
        assert_eq!(wit1.namespace, "wasi");
        assert_eq!(wit1.package, "blobstore");
        assert!(wit1.interfaces.is_empty());
        assert!(wit1.version.is_none());

        // Single interface
        let wit2 = WitInterface::from("wasi:http/incoming-handler");
        assert_eq!(wit2.namespace, "wasi");
        assert_eq!(wit2.package, "http");
        assert_eq!(wit2.interfaces.len(), 1);
        assert!(wit2.interfaces.contains("incoming-handler"));

        // Multiple interfaces
        let wit3 = WitInterface::from("wasi:http/incoming-handler,outgoing-handler,types");
        assert_eq!(wit3.interfaces.len(), 3);
        assert!(wit3.interfaces.contains("incoming-handler"));
        assert!(wit3.interfaces.contains("outgoing-handler"));
        assert!(wit3.interfaces.contains("types"));

        // Just package (no namespace)
        let wit4 = WitInterface::from("mypackage");
        assert_eq!(wit4.namespace, "");
        assert_eq!(wit4.package, "mypackage");
        assert!(wit4.interfaces.is_empty());

        // No namespace with interface
        let wit5 = WitInterface::from("blobstore/types");
        assert_eq!(wit5.namespace, "");
        assert_eq!(wit5.package, "blobstore");
        assert!(wit5.interfaces.contains("types"));
    }

    #[test]
    fn test_parse_with_versions() {
        // Basic version
        let wit1 = WitInterface::from("wasi:blobstore/types@0.2.0");
        assert_eq!(wit1.version, Some(semver::Version::parse("0.2.0").unwrap()));

        // Multiple interfaces with version
        let wit2 = WitInterface::from("wasi:keyvalue/store,atomics,batch@0.2.0-draft");
        assert_eq!(wit2.interfaces.len(), 3);
        assert_eq!(
            wit2.version,
            Some(semver::Version::parse("0.2.0-draft").unwrap())
        );

        // No namespace with version
        let wit3 = WitInterface::from("mypackage/interface1,interface2@1.0.0");
        assert_eq!(wit3.namespace, "");
        assert_eq!(wit3.version, Some(semver::Version::parse("1.0.0").unwrap()));

        // Just package with version
        let wit4 = WitInterface::from("mypackage@1.0.0");
        assert!(wit4.interfaces.is_empty());
        assert_eq!(wit4.version, Some(semver::Version::parse("1.0.0").unwrap()));

        // Prerelease version
        let wit5 = WitInterface::from("wasi:logging/logging@0.1.0-draft");
        assert_eq!(
            wit5.version,
            Some(semver::Version::parse("0.1.0-draft").unwrap())
        );

        // Complex version
        let wit6 = WitInterface::from("wasi:cli/environment@0.2.0-rc.2024-12-05");
        assert_eq!(
            wit6.version,
            Some(semver::Version::parse("0.2.0-rc.2024-12-05").unwrap())
        );

        // Invalid version is ignored
        let wit7 = WitInterface::from("wasi:blobstore/types@invalid-version");
        assert!(wit7.version.is_none());
    }

    #[test]
    fn test_parse_edge_cases() {
        // Spaces in interfaces
        let wit1 = WitInterface::from("wasi:http/incoming-handler, outgoing-handler , types");
        assert_eq!(wit1.interfaces.len(), 3);
        assert!(wit1.interfaces.contains("incoming-handler"));
        assert!(wit1.interfaces.contains("outgoing-handler"));
        assert!(wit1.interfaces.contains("types"));

        // Empty interface segments (trailing comma)
        let wit2 = WitInterface::from("wasi:http/incoming-handler,");
        assert_eq!(wit2.interfaces.len(), 1);
        assert!(wit2.interfaces.contains("incoming-handler"));

        // Leading comma
        let wit3 = WitInterface::from("wasi:http/,incoming-handler");
        assert_eq!(wit3.interfaces.len(), 1);
        assert!(wit3.interfaces.contains("incoming-handler"));

        // Double comma
        let wit4 = WitInterface::from("wasi:http/incoming-handler,,outgoing-handler");
        assert_eq!(wit4.interfaces.len(), 2);

        // Colon in package name (edge case)
        let wit5 = WitInterface::from("foo:bar:baz/interface");
        assert_eq!(wit5.namespace, "foo");
        assert_eq!(wit5.package, "bar:baz");
        assert!(wit5.interfaces.contains("interface"));
    }

    #[test]
    fn test_parse_from_string() {
        let iface: WitInterface = "wasi:http/incoming-handler@0.2.0".into();
        assert_eq!(iface.namespace, "wasi");
        assert_eq!(iface.package, "http");
        assert_eq!(iface.interfaces.len(), 1);
        assert!(iface.interfaces.contains("incoming-handler"));
        assert_eq!(
            iface.version,
            Some(semver::Version::parse("0.2.0").unwrap())
        );

        let iface2: WitInterface = "wasmcloud:messaging".into();
        assert_eq!(iface2.namespace, "wasmcloud");
        assert_eq!(iface2.package, "messaging");
        assert!(iface2.interfaces.is_empty());
        assert_eq!(iface2.version, None);

        let iface3: WitInterface = "wasi:keyvalue/store,atomic@0.1.0".into();
        assert_eq!(iface3.namespace, "wasi");
        assert_eq!(iface3.package, "keyvalue");
        assert_eq!(iface3.interfaces.len(), 2);
        assert!(iface3.interfaces.contains("store"));
        assert!(iface3.interfaces.contains("atomic"));
    }

    #[test]
    fn test_display() {
        let iface = create_interface("wasi", "http", &["incoming-handler"]);
        assert_eq!(format!("{}", iface), "wasi:http/incoming-handler");

        let iface_with_version =
            create_interface_with_version("wasi", "http", &["incoming-handler"], "0.2.0");
        assert_eq!(
            format!("{}", iface_with_version),
            "wasi:http/incoming-handler@0.2.0"
        );

        let iface_no_interfaces = create_interface("wasi", "logging", &[]);
        assert_eq!(format!("{}", iface_no_interfaces), "wasi:logging");
    }

    #[test]
    fn test_display_with_name() {
        let mut iface = create_interface("wasi", "keyvalue", &["store"]);
        iface.name = Some("cache".to_string());
        assert_eq!(format!("{}", iface), "wasi:keyvalue [cache]/store");

        let mut iface_no_interfaces = create_interface("wasi", "keyvalue", &[]);
        iface_no_interfaces.name = Some("sessions".to_string());
        assert_eq!(
            format!("{}", iface_no_interfaces),
            "wasi:keyvalue [sessions]"
        );
    }

    #[test]
    fn test_instance_with_name() {
        let mut iface = create_interface("wasi", "keyvalue", &["store"]);
        assert_eq!(iface.instance(), "wasi:keyvalue");

        iface.name = Some("cache".to_string());
        assert_eq!(iface.instance(), "wasi:keyvalue/cache");

        let mut iface_v = create_interface_with_version("wasi", "keyvalue", &["store"], "0.2.0");
        iface_v.name = Some("sessions".to_string());
        assert_eq!(iface_v.instance(), "wasi:keyvalue/sessions@0.2.0");
    }

    #[test]
    fn test_merge_with_matching_name() {
        let mut iface1 = create_interface("wasi", "keyvalue", &["store"]);
        iface1.name = Some("cache".to_string());

        let mut iface2 = create_interface("wasi", "keyvalue", &["atomics"]);
        iface2.name = Some("cache".to_string());

        assert!(iface1.merge(&iface2));
        assert!(iface1.interfaces.contains("store"));
        assert!(iface1.interfaces.contains("atomics"));
    }

    #[test]
    fn test_merge_with_different_name() {
        let mut iface1 = create_interface("wasi", "keyvalue", &["store"]);
        iface1.name = Some("cache".to_string());

        let mut iface2 = create_interface("wasi", "keyvalue", &["store"]);
        iface2.name = Some("sessions".to_string());

        // Different names => different instances => merge fails
        assert!(!iface1.merge(&iface2));
    }

    #[test]
    fn test_merge_named_vs_unnamed() {
        let mut iface1 = create_interface("wasi", "keyvalue", &["store"]);
        // iface1 has no name (None)

        let mut iface2 = create_interface("wasi", "keyvalue", &["store"]);
        iface2.name = Some("cache".to_string());

        // One named, one unnamed => different instances => merge fails
        assert!(!iface1.merge(&iface2));
    }

    #[test]
    fn test_contains_unnamed_provider_matches_named_request() {
        // An unnamed provider satisfies a named request (name acts as wildcard when absent)
        let provider = create_interface("wasi", "keyvalue", &["store", "atomics"]);

        let mut named_req = create_interface("wasi", "keyvalue", &["store"]);
        named_req.name = Some("cache".to_string());

        assert!(provider.contains(&named_req));
    }

    #[test]
    fn test_contains_matching_names() {
        // When both interfaces have the same name, contains should match
        let mut a = create_interface("wasi", "keyvalue", &["store", "atomics"]);
        a.name = Some("cache".to_string());

        let mut b = create_interface("wasi", "keyvalue", &["store"]);
        b.name = Some("cache".to_string());

        assert!(a.contains(&b));
    }

    #[test]
    fn test_contains_different_names() {
        // When both interfaces have different names, contains should not match
        let mut a = create_interface("wasi", "keyvalue", &["store", "atomics"]);
        a.name = Some("cache".to_string());

        let mut b = create_interface("wasi", "keyvalue", &["store"]);
        b.name = Some("sessions".to_string());

        assert!(!a.contains(&b));
    }

    #[test]
    fn test_hash_includes_name() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut iface1 = create_interface("wasi", "keyvalue", &["store"]);
        let mut iface2 = create_interface("wasi", "keyvalue", &["store"]);
        iface2.name = Some("cache".to_string());

        let hash = |i: &WitInterface| {
            let mut h = DefaultHasher::new();
            i.hash(&mut h);
            h.finish()
        };

        // Different names should produce different hashes
        assert_ne!(hash(&iface1), hash(&iface2));

        // Same name should produce the same hash
        iface1.name = Some("cache".to_string());
        assert_eq!(hash(&iface1), hash(&iface2));
    }
}
