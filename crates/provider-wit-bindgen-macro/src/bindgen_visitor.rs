//! Visitor(s) built to traverse output of upstream bindgen (wasmtime::component macro) and extract information,
//! structs, and types for use in performing provider bindgen for wasmCloud.

use std::collections::HashMap;

use heck::ToUpperCamelCase;
use proc_macro2::{Ident, Span, TokenStream, TokenTree};
use quote::{ToTokens, TokenStreamExt};
use syn::{
    parse_quote,
    punctuated::Punctuated,
    visit_mut::{visit_item_mut, VisitMut},
    FnArg, Generics, ImplItem, ImplItemFn, Item, ItemMod, ItemType, PathSegment, ReturnType, Token,
    Type,
};
use tracing::{debug, trace, warn};

use crate::rust::{convert_to_owned_type_arg, count_preceeding_supers};
use crate::wit::extract_witified_map;
use crate::{
    EnumLookup, LatticeExposedInterface, ProviderBindgenConfig, StructLookup, TypeLookup,
    WitInterfacePath, WitNamespaceName, WitPackageName, EXPORTS_MODULE_NAME,
};

/// Path to a module with functions that were exported in Rust code,
/// normally *without* the 'exports' module near the top (ex. wasmcloud.keyvalue.key_value)
type ExportModulePath = String;

/// A struct for visiting the output of wit-bindgen
/// focused around gathering all the important declarations we care about
#[derive(Default)]
pub(crate) struct WitBindgenOutputVisitor {
    /// Whether to replace WIT-ified maps (`list<tuple<T,T>>`) with a map type (i.e. `std::collections::HashMap`)
    pub(crate) replace_witified_maps: bool,

    /// WIT namespace
    pub(crate) wit_ns: Option<WitNamespaceName>,

    /// WIT package
    pub(crate) wit_pkg: Option<WitPackageName>,

    /// Parents of the current module being traversed
    pub(crate) parents: Vec<Ident>,

    /// Top level module that contains all WIT exports
    /// normally with internal modules starting from namespace
    /// ex. ('exports' -> <WIT namespace> -> <WIT pkg>)
    pub(crate) exports_ns_module: Option<ItemMod>,

    /// List of interfaces that if specified, will only be exposed on the lattice.
    /// If left empty, this indicates that all interfaces should be exposed
    pub(crate) exposed_interface_allow_list: Vec<LatticeExposedInterface>,

    /// List of interfaces that should explicitly not be exposed on the lattice
    pub(crate) exposed_interface_deny_list: Vec<LatticeExposedInterface>,

    /// Structs that were modified and extended to derive Serialize/Deserialize
    pub(crate) serde_extended_structs: StructLookup,

    /// Enums that were modified and extended to derive Serialize/Deserialize
    pub(crate) serde_extended_enums: EnumLookup,

    /// Lookup of encountered types that were produced by bindgen, with their fully qualified names
    pub(crate) type_lookup: TypeLookup,

    /// Functions in traits that are exported out onto the lattice, which we
    /// will have to listen for
    pub(crate) export_trait_methods: HashMap<WitInterfacePath, Vec<ImplItemFn>>,
}

impl WitBindgenOutputVisitor {
    /// Build a new visitor to traverse a wit-bindgen generated syntax tree
    #[must_use]
    pub(crate) fn new(cfg: &ProviderBindgenConfig) -> Self {
        Self {
            wit_ns: cfg.wit_ns.clone(),
            wit_pkg: cfg.wit_pkg.clone(),
            exposed_interface_allow_list: cfg.exposed_interface_allow_list.clone(),
            exposed_interface_deny_list: cfg.exposed_interface_deny_list.clone(),
            replace_witified_maps: cfg.replace_witified_maps,
            ..Default::default()
        }
    }

    /// Check the distance of the current module from crate/generated wit-bindgen content root
    fn current_module_level(&self) -> usize {
        self.parents.len()
    }

    /// Get the full path to the current module, excluding `exports`
    /// ex. (`<namespace>::<package>::some::wit::interface`)
    fn generate_export_path(&self) -> ExportModulePath {
        self.parents
            .iter()
            .filter_map(|i| {
                let name = i.to_string();
                if name == "exports" {
                    None
                } else {
                    Some(name)
                }
            })
            .collect::<Vec<String>>()
            .join(".")
    }

    /// Get the name of the current module (e.x. `interface`)
    fn current_module_name(&self) -> Option<String> {
        self.parents.last().map(ToString::to_string)
    }

    /// Check if a given string is the same as the top-level WIT namespace that was detected
    fn is_wit_ns(&self, s: impl AsRef<str>) -> bool {
        if let Some(v) = &self.wit_ns {
            v == s.as_ref()
        } else {
            false
        }
    }

    /// Check whether a the current node is directly under the wasm namespace
    /// Primarily used for detecting the package
    /// i.e. '<ns>/<package>'
    fn at_wit_ns_module_child(&self) -> bool {
        self.parents
            .last()
            .is_some_and(|ps| self.is_wit_ns(ps.to_string()))
    }

    /// Check whether the direct parent has a given name value
    fn at_child_of_module(&self, name: impl AsRef<str>) -> bool {
        self.parents.last().is_some_and(|v| v == name.as_ref())
    }

    /// Check whether we are currently at a module *below* the 'exports' known module name
    fn at_exported_module(&self) -> bool {
        self.parents.iter().any(|v| v == EXPORTS_MODULE_NAME)
    }

    /// Check whether the current path matches any known WASI built-ins (ex. wasi::io)
    ///
    /// WASI built-ins usually need to be ignored by bindgen
    fn is_wasi_builtin(&self) -> bool {
        for builtin in [("wasi", "io")] {
            match (
                self.parents.iter().position(|v| v == builtin.0),
                self.parents.iter().position(|v| v == builtin.1),
            ) {
                (Some(n), Some(n1)) if n1 == n + 1 => {
                    // If we see the path specified above consecutively, we know
                    // that we're in the path of a builtin
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Check whether the current path matches any known wasmcloud local-only built-ins (ex. wasmcloud::bus::host)
    ///
    /// Structs/Enums/etc in the hierarchy that match this cannot be sent across the lattice,
    /// thus generation should generally not be done for them
    fn is_wasmcloud_local_only_builtin(&self) -> bool {
        for builtin in [("wasmcloud", "bus", "host")] {
            match (
                self.parents.iter().position(|v| v == builtin.0),
                self.parents.iter().position(|v| v == builtin.1),
                self.parents.iter().position(|v| v == builtin.2),
            ) {
                (Some(n), Some(n1), Some(n2)) if n2 == n1 + 1 && n1 == n + 1 => {
                    // If we see the path specified above consecutively, we know
                    // that we're in the path of a builtin
                    return true;
                }
                _ => {}
            }
        }
        false
    }
}

impl VisitMut for WitBindgenOutputVisitor {
    fn visit_item_mod_mut(&mut self, node: &mut ItemMod) {
        debug!(
            "(bindgen module hierarchy): {}> {}",
            "=".repeat(self.current_module_level()),
            node.ident
        );

        // Detect the WIT namespace while traversing the bindgen output
        //
        // We expect the top level rust module (i.e. level zero of the module hierarchy)
        // in a package with imports to be the wit namespace.
        //
        // Packages with exports (which may *only* have exports) have 'exports' at level zero
        // then follow a similar pattern
        if self.wit_ns.is_none()
            && ((self.current_module_level() == 0 && node.ident != EXPORTS_MODULE_NAME)
                || (self.current_module_level() == 1 && self.at_exported_module()))
        {
            self.wit_ns = Some(node.ident.to_string());
        }

        // Detect the WIT package while traversing the bindgen output
        //
        // We expect the second level rust module (i.e. level 1 in a zero-indexed module hierarchy)
        // in a package with imports to be the wit package name.
        //
        // Packages with exports (which may *only* have exports) have 'exports' at level zero
        // then follow a similar pattern. For example, one would expect a module hierarchy like
        // `exports -> <wit namespace> -> <wit package>`
        if (
            self.current_module_level() == 1
            && self.at_wit_ns_module_child()
            && !self.at_exported_module())
            // Exports only case
            || (self.current_module_level() == 2 && self.at_exported_module())
        {
            self.wit_pkg = Some(node.ident.to_string());
        }

        // Recognize the 'exports' module which contains
        // all the exported interfaces
        //
        // ASSUMPTION: all exported modules are put into a level 0 'exports' module
        // which contains the top level namespace again
        if self.current_module_level() == 1 && self.at_child_of_module(EXPORTS_MODULE_NAME) {
            // this would be the ('exports' -> <ns>) node, not 'exports' itself.
            self.exports_ns_module = Some(node.clone());
        }

        // ASSUMPTION: level 2 modules contain externally visible *or* used interfaces
        // (i.e. ones that are exported)
        // 'use' calls will  cause an interface to show up, but only if the
        // thing that uses it is imported/exported

        // Recur/Traverse deeper into the detected modules where possible
        if let Some((_, ref mut items)) = &mut node.content {
            // Save the current module before we go spelunking
            self.parents.push(node.ident.clone());
            for item in items {
                self.visit_item_mut(item);
            }
            self.parents.pop();
        }
    }

    fn visit_item_mut(&mut self, node: &mut syn::Item) {
        match node {
            // Interfaces exported in the WIT represent the messages that we must listen for on the lattice.
            // wasmtime_component_macro turns the exported interfaces into a shape that looks like this:
            // ```
            // pub struct KeyValue {
            //     contains: wasmtime::component::Func,
            //     ...
            //     set: wasmtime::component::Func,
            // }
            // impl KeyValue {
            //     pub fn new( ... ) -> wasmtime::Result<KeyValue> { ... }
            //
            //     pub fn call_contains<S: wasmtime::AsContextMut>(
            //         &self,
            //         mut store: S,
            //         arg0: &str,
            //     ) -> wasmtime::Result<bool> { ... }
            //
            //     pub fn call_del<S: wasmtime::AsContextMut>(
            //         &self,
            //         mut store: S,
            //         arg0: &str,
            //     ) -> wasmtime::Result<bool> { ...}
            // ```
            //
            // All the functions exported by the interface are present, but they are opaque in their requirements,
            // they are all simply `wasmtime::component::Func`s.
            //
            // The functions all get converted into `call_<method name>` functions on the impl
            // of the interface (along with a "new" method).
            //
            // To get *back* to the functions we actually want to call here (and the rust types for their inputs),
            // we can process the functions (except `new`) and look at args and results.
            //
            Item::Impl(i) => 'visit_impl: {
                // If the impl we're looking at is is under a WASI built-in package (ex. wasi:io),
                // we don't want to include it for any kind of post processing
                if self.is_wasi_builtin() {
                    break 'visit_impl;
                }

                let impl_type_name = i.self_ty.to_token_stream();
                trace!("visiting impl: {}", impl_type_name);

                // The impl blocks that we're looking for are standalone, not trait impls
                if i.trait_.is_some() {
                    break 'visit_impl;
                }

                // Retrieve the interface name from the module hierarchy (immediate parent)
                //
                // If we're missing a parent, then we're likely at the top level, which does not
                // contain impls we want to process
                let iface = if let Some(iface) = self.parents.last() {
                    iface
                } else {
                    break 'visit_impl;
                };

                // Retrieve the WIT namespace for this impl
                let wit_ns = self
                    .parents
                    .get(self.parents.len() - 3)
                    .unwrap_or_else(|| {
                        panic!("unexpectedly missing ns level package (2 up from [{iface}] in generated bindgen code)")
                    })
                    .to_string();

                // Retrieve the WIT package for this impl
                let wit_pkg = self
                    .parents
                    .get(self.parents.len() - 2)
                    .unwrap_or_else(|| {
                        panic!("unexpectedly missing ns level package (1 up from [{iface}] in generated bindgen code)")
                    })
                    .to_string();

                // Rebuild the WIT interface name
                let full_iface_name = format!("{wit_ns}:{wit_pkg}/{iface}");

                // Check if we should ignore this interface based on allow/deny lists
                if should_ignore_interface(
                    self.exposed_interface_allow_list.as_slice(),
                    self.exposed_interface_deny_list.as_slice(),
                    &full_iface_name,
                    &(wit_ns, wit_pkg, iface.to_string()),
                ) {
                    return;
                }

                // For every function, we should be generating a relevant export trait method
                for item in &i.items {
                    if let ImplItem::Fn(f) = item {
                        let fn_name = f.sig.ident.to_string();

                        // Skip the "new" function for the function-holding struct,
                        // only look at functions that stat with "call_" as they're the ones that
                        // govern calling the `wasmtime::component::Func`s that represent exported fns
                        if fn_name == "new" || !fn_name.starts_with("call_") {
                            debug!("skipping function [new] for impl [{impl_type_name}] which should be part of iface [{full_iface_name}]");
                            continue;
                        }

                        // Clone the function so we can modify it and trim it
                        let mut trimmed_fn = f.clone();

                        // Remove "call_" prefix from fn name, skip if we somehow don't have it (we should)
                        trimmed_fn.sig.ident = if let Some(stripped) = fn_name.strip_prefix("call_")
                        {
                            Ident::new(stripped, trimmed_fn.sig.ident.span())
                        } else {
                            warn!("unexpectedly missing 'call_' prefix on function [{fn_name}] for impl [{impl_type_name}] which should be part of iface [{full_iface_name}]");
                            continue;
                        };

                        // Trim the first 2 arguments which we know will be `self` and `mut store: Store`
                        // the rest will be the actual rust inputs of the function
                        trimmed_fn.sig.inputs = <Punctuated<FnArg, Token![,]>>::from_iter(
                            trimmed_fn.sig.inputs.into_iter().skip(2),
                        );

                        // Remove generics from the fn (we expect the only generic clause to be `<S: wasmtime::AsContextMut>`)
                        trimmed_fn.sig.generics = Generics::default();

                        // Convert the types used in the export functions into types that can be used from the trait function
                        //
                        // (ex. str -> String, [String] -> Vec<String>)
                        for fn_arg in trimmed_fn.sig.inputs.iter_mut() {
                            let (_arg_name, converted_tokens) = convert_to_owned_type_arg(
                                &self.serde_extended_structs,
                                &self.type_lookup,
                                fn_arg,
                                self.replace_witified_maps,
                            );

                            let updated_fn_arg = syn::parse2::<FnArg>(converted_tokens)
                                .expect("failed to produce valid FnArg from owned type conversion");

                            trace!(
                                "fn_arg [{}] converted to [{}]",
                                fn_arg.to_token_stream().to_string(),
                                updated_fn_arg.to_token_stream().to_string()
                            );

                            *fn_arg = updated_fn_arg;
                        }

                        // Get the return type of the function, we expect it to be a wasmtime::Result<T>
                        let output_type = trimmed_fn
                            .sig
                            .output
                            .to_token_stream()
                            .into_iter()
                            .collect::<Vec<TokenTree>>();

                        // If we're dealing with a wasmtime::Result<T> type as we expect then we should pull out the T
                        // so we can use them standalone as a return type
                        let inner_tokens = match extract_wasmtime_result_type(&output_type) {
                            Some(t) => t,
                            None => {
                                debug!("skipping function [{fn_name}] in impl [{impl_type_name}] which does not return a wasmtime::Result");
                                continue;
                            }
                        };

                        // Modify the functions output type
                        trimmed_fn.sig.output =
                            syn::parse2::<ReturnType>(quote::quote!(-> #inner_tokens))
                                .expect("failed to purge wasmtime::Result from method return");

                        // Save the manipulated function signature as a method, which will be put in a trait
                        // that the provider must implement
                        let full_path = self.generate_export_path();
                        debug!(
                                "adding exported trait method for path [{full_path}], trimmed signature: [{}]",
                                trimmed_fn.sig.to_token_stream().to_string(),
                            );
                        self.export_trait_methods
                            .entry(full_path)
                            .or_default()
                            .push(trimmed_fn);
                    }
                }
            }

            // Process type declarations that appear in bindgen output
            //
            // Primarily, we pick up the definitions here so that we can use them for full qualification later
            Item::Type(t) => 'visit_type: {
                if self.is_wasi_builtin() {
                    break 'visit_type;
                }
                trace!("visiting type: {}", t.ident);

                // Determine the import path to this type
                let mut import_path = Punctuated::<syn::PathSegment, Token![::]>::new();
                for p in self.parents.iter() {
                    import_path.push(syn::PathSegment::from(p.clone()));
                }
                import_path.push(syn::PathSegment::from(t.ident.clone()));

                // For types generated due to dependencies in WIT (ex. wasm internals like wasi::io::stream, wasmcloud::bus::lattice)
                // we must replace their convoluted (`super::...` prefixes with `crate::`)
                let mut cloned_t = t.clone();
                let ItemType {
                    ty: ref mut item_ty,
                    ..
                } = cloned_t;

                // If the type alias that we're about to process has `super::`s attached, we need to translate those
                // to the actual types they *should* be, which are likely hanging off the crate or some other
                // dep like `wasmtime` (ex. `wasmtime::component::Resource`)
                let preceeding_super_count = count_preceeding_supers(item_ty.as_ref());
                if preceeding_super_count > 0 {
                    if let Type::Path(ty_path) = item_ty.as_mut() {
                        // Create a cloned version fo the original path to use for modifications
                        let cloned_ty_path = ty_path.clone();
                        // Clear out the segments on the original type path
                        ty_path.path.segments.clear();

                        // Push in `crate`
                        ty_path
                            .path
                            .segments
                            .push_value(PathSegment::from(quote::format_ident!("crate")));
                        ty_path
                            .path
                            .segments
                            .push_punct(Token![::](Span::call_site()));

                        // Push in all non-"super" segments
                        cloned_ty_path
                            .path
                            .segments
                            .iter()
                            .filter(|s| s.ident != "super")
                            .for_each(|s| {
                                if !ty_path.path.segments.empty_or_trailing() {
                                    ty_path
                                        .path
                                        .segments
                                        .push_punct(Token![::](Span::call_site()));
                                }
                                ty_path.path.segments.push_value(s.clone());
                            });
                    }
                };

                // We should only add this type to the type lookup if it is *not* already a processed struct
                // or enum, as those will be output at the top level in the bind-gen'd code.
                //
                // Having both the type declaration and the top level struct/enum declaration would cause a conflict
                if !self.serde_extended_enums.contains_key(&t.ident.to_string())
                    && !self
                    .serde_extended_structs
                    .contains_key(&t.ident.to_string())
                // We exclude built-in wasi types here because they *should*
                // be implemented & brought in as enums/structs
                    && !self.is_wasi_builtin()
                // If this type alias has no preceeding `super::` count and it has not been seen, it's most likely the
                // resolved alias to a basic Rust type:
                // ```
                // type T = vec<u8>
                // ```
                //
                // Otherwise, if there *is* a preceeding `super::` count, it likely looks like this:
                // ```
                // type T = super::some::dep::T
                // ```
                // We should avoid overwriting the basic Rust type alias, since that one should be hoisted to the top.
                // All code will deal with the types that the top level(i.e. generated code will contain `T`, not `super::some::dep::T`)
                // otherwise, we can add if it's not overlapping with an existing entry.
                    && (preceeding_super_count == 0
                    || !self.type_lookup.contains_key(&t.ident.to_string()))
                {
                    // Add the type to the lookup so it can be used later for fully qualified names
                    self.type_lookup
                        .insert(t.ident.to_string(), (import_path, cloned_t));
                }
            }

            Item::Enum(e) => {
                trace!("visiting enum: {}", e.ident);

                // If this is a generated enum (from a WIT record), add serde Serialize/Deserialize
                //
                // NOTE: we MUST allow in built-in wasi enums, since they are used by higher level code
                //
                // Exclude top level structs, since they indirectly include the exported module
                if self.current_module_level() != 0
                    // Ensure the enum has not already been processed
                    && !self.serde_extended_enums.contains_key(&e.ident.to_string())
                    // Ensure that the enum is not already aliased to something else
                    // enums that are aliases have types that are already defined elsewhere
                    && !self.type_lookup.contains_key(&e.ident.to_string())
                {
                    // Clear all pre-existing attributes (i.e. [component])
                    e.attrs.clear();

                    // Clear all pre-existing attributes from fields (mostly [component])
                    for v in &mut e.variants {
                        v.attrs.clear();

                        // Process all fields in every variant to perform standard replacements
                        for f in &mut v.fields {
                            // If the type of a particular field is a Vec<u8>,
                            // opt in to serde's specialized handling since this is what the
                            // implementation written in the host currently expects
                            if f.ty == syn::parse_str::<Type>("Vec<u8>").expect("failed to parse") {
                                f.attrs.push(parse_quote!(#[serde(with = "::wasmcloud_provider_wit_bindgen::deps::serde_bytes")]));
                            }

                            // If an enum contains a type that is a resource (i.e. a wasmtime::component::Resource),
                            // we can't actually send that across the lattice, we can only send a *reference* to it.
                            //
                            // For now, resources are converted to u32s (i.e. their `rep()` or pointer), and sent across the lattice that way.
                            match &f
                                .ty
                                .to_token_stream()
                                .into_iter()
                                .collect::<Vec<TokenTree>>()[..] {
                                    [
                                        TokenTree::Ident(w), // wasmtime
                                        TokenTree::Punct(_), // :
                                        TokenTree::Punct(_), // :
                                        TokenTree::Ident(w1), // component
                                        TokenTree::Punct(_), // :
                                        TokenTree::Punct(_), // :
                                        TokenTree::Ident(w2), // Resource
                                        TokenTree::Punct(b1), // <
                                        _inner @ ..,
                                        TokenTree::Punct(b2), // >
                                    ] if w == "wasmtime"
                                        && w1 == "component"
                                        && w2 == "Resource"
                                        && b1.to_string() == "<"
                                        && b2.to_string() == ">" => {
                                        f.ty = syn::parse_str::<Type>("u32").expect("failed to parse");
                                    }
                                    _ => {}
                                }

                            // If the struct field is a WIT-ified map, then we should replace
                            // it with a proper hash map type
                            if self.replace_witified_maps
                                && f.ident
                                    .as_ref()
                                    .is_some_and(|i| i.to_string().ends_with("_map"))
                            {
                                if let Some(map_type) = extract_witified_map(
                                    &f.ty
                                        .to_token_stream()
                                        .into_iter()
                                        .collect::<Vec<TokenTree>>(),
                                ) {
                                    f.ty = parse_quote!(#map_type);
                                    f.ident = f.ident.as_mut().map(|i| {
                                        Ident::new(i.to_string().trim_end_matches("_map"), i.span())
                                    });
                                }
                            }
                        }
                    }

                    // Add the attributes we want to be present to the enum
                    e.attrs.append(&mut vec![
                        parse_quote!(
                            #[derive(Debug, ::wasmcloud_provider_wit_bindgen::deps::serde::Serialize, ::wasmcloud_provider_wit_bindgen::deps::serde::Deserialize, ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive::Encode, ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive::Receive, ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive::Subscribe)]
                        ),
                        parse_quote!(
                            #[serde(crate = "::wasmcloud_provider_wit_bindgen::deps::serde")]
                        ),
                        parse_quote!(
                            #[wrpc_transport_derive(crate = "::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive")]
                        ),
                    ]);

                    // Save the enum by name to the tally of structs that have been extended
                    // this is used later to generate interfaces, when generating interfaces, as a import path lookup
                    // so that types can be resolved (i.e. T -> path::to::T)
                    let mut import_path = Punctuated::<syn::PathSegment, Token![::]>::new();
                    for p in self.parents.iter() {
                        import_path.push(syn::PathSegment::from(p.clone()));
                    }
                    import_path.push(syn::PathSegment::from(e.ident.clone()));

                    // Disallow the case where two identically named enums exist under different paths
                    if self.serde_extended_enums.contains_key(&e.ident.to_string()) {
                        panic!("found duplicate instances of enum [${}]", e.ident);
                    }

                    trace!("adding serde extended enum [{}]", e.ident.to_string());

                    self.serde_extended_enums
                        .insert(e.ident.to_string(), (import_path, e.clone()));
                }
            }

            // Process struct declarations that appear in the bindgen output
            Item::Struct(s) => 'visit_struct: {
                if self.is_wasi_builtin() {
                    break 'visit_struct;
                }

                // Skip the "Guest" struct which is generated by bindgen, and contains all the exported functions.
                //
                // Normally this struct is similar to the InvocationHandler that we generate,
                // it contains all the functions that are callable, as `wasmtime::component::Func`s
                //
                // For example, for `wasmcloud:keyvalue`:
                //
                // ```
                // pub struct Guest {
                //   contains: wasmtime::component::Func,
                //   del: wasmtime::component::Func,
                //   ..
                // }
                // ```
                if self.at_exported_module() && s.ident == "Guest" {
                    break 'visit_struct;
                }

                trace!("visiting struct: [{}]", s.ident);

                // If this is a generated struct (from a WIT record), add serde Serialize/Deserialize
                // exclude top level structs, since they indirectly include the exported module
                if self.current_module_level() != 0
                    && !self
                        .serde_extended_structs
                        .contains_key(&s.ident.to_string())
                    // We must exclude structs that are wasmcloud builtins, since we know some of them to be
                    // impossible to pass over the lattice in a  easy manner
                    && !self.is_wasmcloud_local_only_builtin()
                    // Exclude structs that are named exactly the same as the module,
                    // since that's the struct that we'll be replacing with the InvocationHandler
                    //
                    // Normally that module-named struct contains wasmtime::component::Func
                    // which cannot be Serialized
                    && !self.current_module_name().is_some_and(|m| s.ident == m.to_upper_camel_case())
                {
                    // Clear all pre-existing attributes (i.e. [component])
                    s.attrs.clear();

                    // Clear all pre-existing attributes from fields (mostly [component])
                    for f in &mut s.fields {
                        f.attrs.clear();

                        // If the type of a particular field is a Vec<u8>,
                        // opt in to serde's specialized handling since this is what the
                        // implementation written in the host currently expects
                        if f.ty == syn::parse_str::<Type>("Vec<u8>").expect("failed to parse") {
                            f.attrs.push(parse_quote!(#[serde(with = "::wasmcloud_provider_wit_bindgen::deps::serde_bytes")]));
                        }

                        // If the struct field is a WIT-ified map, then we should replace
                        // it with a proper hash map type
                        if self.replace_witified_maps
                            && f.ident
                                .as_ref()
                                .is_some_and(|i| i.to_string().ends_with("_map"))
                        {
                            if let Some(map_type) = extract_witified_map(
                                &f.ty
                                    .to_token_stream()
                                    .into_iter()
                                    .collect::<Vec<TokenTree>>(),
                            ) {
                                f.ty = parse_quote!(#map_type);
                                f.ident = f.ident.as_mut().map(|i| {
                                    Ident::new(i.to_string().trim_end_matches("_map"), i.span())
                                });
                            }
                        }
                    }

                    // Add the attributes we want to be present
                    s.attrs.append(&mut vec![
                        parse_quote!(
                            #[derive(Debug, ::wasmcloud_provider_wit_bindgen::deps::serde::Serialize, ::wasmcloud_provider_wit_bindgen::deps::serde::Deserialize, ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive::Encode, ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive::Receive, ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive::Subscribe)]
                        ),
                        parse_quote!(
                            #[serde(crate = "::wasmcloud_provider_wit_bindgen::deps::serde", rename_all = "camelCase")]
                        ),
                        parse_quote!(
                            #[wrpc_transport_derive(crate = "::wasmcloud_provider_wit_bindgen::deps::wrpc_transport_derive")]
                        ),
                    ]);

                    // Save the Struct by name to the tally of structs that have been extended
                    // this is used later to generate interfaces, when generating interfaces, as a import path lookup
                    // so that types can be resolved (i.e. T -> path::to::T)
                    let mut struct_import_path = Punctuated::<syn::PathSegment, Token![::]>::new();

                    // Add all parents until this point to the internal struct's name
                    for p in self.parents.iter() {
                        struct_import_path.push(syn::PathSegment::from(p.clone()));
                    }
                    // Add the struct name itself
                    struct_import_path.push(syn::PathSegment::from(s.ident.clone()));

                    // Disallow the case where two identically named structs exist under different paths
                    if self
                        .serde_extended_structs
                        .contains_key(&s.ident.to_string())
                    {
                        panic!("found duplicate instances of struct [${}]", s.ident);
                    }

                    trace!("adding serde extended struct [{}]", s.ident.to_string());

                    self.serde_extended_structs
                        .insert(s.ident.to_string(), (struct_import_path, s.clone()));
                }
            }

            _ => visit_item_mut(self, node),
        }
    }
}

/// Check whether a WIT interface should be ignored, based on interface allow/deny lists
/// (normally provided in the bindgen config)
fn should_ignore_interface(
    allow_list: impl AsRef<[LatticeExposedInterface]>,
    deny_list: impl AsRef<[LatticeExposedInterface]>,
    interface_name: impl AsRef<str>,
    interface: &LatticeExposedInterface,
) -> bool {
    let allow_list = allow_list.as_ref();
    let deny_list = deny_list.as_ref();
    let interface_name = interface_name.as_ref();
    // Use the allow and deny lists to determine which interfaces should be processed
    match (allow_list, deny_list) {
        // If neither allow nor deny were specified, we are unconstrained
        ([], []) => {
            debug!("processing interface [{interface_name}], unconstrained (no allow/deny list)");
        }
        // If allow list is present (and deny missing), process only allow list
        (allow, []) => {
            if allow.contains(interface) {
                debug!("processing interface [{interface_name}], included in allow list");
            } else {
                warn!("skipping interface [{interface_name}], missing from allow list");
                return true;
            }
        }
        // If deny list is present (and allow missing), process only deny list
        ([], deny) => {
            if deny.contains(interface) {
                warn!("skipping interface [{interface_name}], included in deny list");
                return true;
            } else {
                debug!("processing interface [{interface_name}], not included in deny list");
            }
        }
        // If both allow and deny are present, process allow then deny
        (allow, deny) => {
            if allow.contains(interface) && !deny.contains(interface) {
                debug!(
                    "processing interface [{interface_name}], included in allow and not in deny"
                );
            } else {
                warn!("[warn] skipping interface [{interface_name}], not included in allow or missing from deny");
                return true;
            }
        }
    };

    // By default, don't ignore the interface
    false
}

/// Extract the T from a `wasmtime::Result<T, Error>`, if the tree of tokens does
/// represent a `wasmtime::Result`
fn extract_wasmtime_result_type(tts: &[TokenTree]) -> Option<TokenStream> {
    match tts[..] {
        [
            TokenTree::Punct(_), // -
            TokenTree::Punct(_), // >
            TokenTree::Ident(ref w), // wasmtime
            TokenTree::Punct(_), // :
            TokenTree::Punct(_), // :
            TokenTree::Ident(ref r), // Result
            TokenTree::Punct(_), // <
            .., // T
            TokenTree::Punct(_), // >
        ] if w == "wasmtime" && r == "Result" => {
            // Build a TokenStream that represents the T type
            Some(tts[7..tts.len() - 1].iter().fold(TokenStream::new(), |mut acc, v| {
                acc.append(v.clone());
                acc
            }))
        },
        _ => None,
    }
}
