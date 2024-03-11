use proc_macro2::Span;
use syn::{braced, parse::Parse, punctuated::Punctuated, Ident, LitStr, Token};

use crate::vendor::wasmtime_component_macro::bindgen::Config as WitBindgenConfig;
use crate::wit::{WitFnList, WitNamespaceName, WitPackageName};
use crate::{ImplStructName, LatticeExposedInterface, WasmcloudContract};

/// Inputs to the wit_bindgen_wasmcloud::provider::binary::generate! macro
pub(crate) struct ProviderBindgenConfig {
    /// The struct that will contain the implementation of the provider
    pub(crate) impl_struct: ImplStructName,

    /// The wasmCloud contract that the provider fulfills
    pub(crate) contract: WasmcloudContract,

    /// WIT namespace of the provider WIT
    pub(crate) wit_ns: Option<WitNamespaceName>,

    /// WIT package of the provider
    pub(crate) wit_pkg: Option<WitPackageName>,

    /// Interfaces that will be exposed onto the lattice.
    /// If left empty, all available interfaces will be exposed
    pub(crate) exposed_interface_allow_list: Vec<LatticeExposedInterface>,

    /// Interfaces that must explicitly not be exposed onto the lattice
    pub(crate) exposed_interface_deny_list: Vec<LatticeExposedInterface>,

    /// wit-bindgen configuration that is passed straight through (uses vendored wit-bindgen)
    ///
    /// During an actual parse run, configuration to pass through to wit bindgen *must* be provided
    /// this means that full parse runs can only be run when you can set up the filesystem as necessary.
    ///
    /// This allows local tests to easily generate `ProviderBindgenConfig` objects, without building the
    /// file tree (and related WIT) that wit-bindgen would attempt to read
    pub(crate) wit_bindgen_cfg: Option<WitBindgenConfig>,

    /// Whether to replace WIT-ified maps (`list<tuple<T, T>>`) with a Map type (`std::collections::HashMap`)
    pub(crate) replace_witified_maps: bool,
}

impl Parse for ProviderBindgenConfig {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let call_site = Span::call_site();
        // Ensure the configuration starts with '{' as it should be an object
        if !input.peek(syn::token::Brace) {
            return Err(syn::Error::new::<String>(
                call_site,
                "bindgen configuration should start with a brace ('{')".into(),
            ));
        }

        // Expect and parse a braced set of bindgen configuration options
        let values;
        braced!(values in input);
        let entries =
            Punctuated::<ProviderBindgenConfigOption, Token![,]>::parse_terminated(&values)?;

        // Gather members of bindgen
        let mut contract: Option<WasmcloudContract> = None;
        let mut impl_struct: Option<ImplStructName> = None;
        let mut wit_ns: Option<WitNamespaceName> = None;
        let mut wit_pkg: Option<WitPackageName> = None;
        let mut wit_bindgen_cfg: Option<WitBindgenConfig> = None;
        let mut exposed_interface_allow_list: Option<WitFnList> = None;
        let mut exposed_interface_deny_list: Option<WitFnList> = None;
        let mut replace_witified_maps: bool = false;

        // For each successfully parsed configuration entry in the map, build the appropriate bindgen option
        for entry in entries.into_pairs() {
            match entry.into_value() {
                ProviderBindgenConfigOption::Contract(c) => {
                    contract = Some(c.value());
                }
                ProviderBindgenConfigOption::WitNamespace(ns) => {
                    wit_ns = Some(ns.value());
                }
                ProviderBindgenConfigOption::WitPackage(pkg) => {
                    wit_pkg = Some(pkg.value());
                }
                ProviderBindgenConfigOption::ExposedFnAllowList(list) => {
                    exposed_interface_allow_list = Some(list)
                }
                ProviderBindgenConfigOption::ExposedFnDenyList(list) => {
                    exposed_interface_deny_list = Some(list)
                }
                ProviderBindgenConfigOption::ImplStruct(s) => impl_struct = Some(s.to_string()),
                ProviderBindgenConfigOption::WitBindgenCfg(cfg) => {
                    wit_bindgen_cfg = Some(cfg);
                }
                ProviderBindgenConfigOption::ReplaceWitifiedMaps(opt) => {
                    replace_witified_maps = opt.value();
                }
            }
        }

        // Build the bindgen configuration from the parsed parts
        syn::Result::Ok(ProviderBindgenConfig {
            impl_struct: impl_struct.ok_or_else(|| {
                syn::Error::new(
                    call_site,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing/invalid 'impl_struct' bindgen option",
                    ),
                )
            })?,
            contract: contract.ok_or_else(|| {
                syn::Error::new(
                    call_site,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing/invalid 'contract' bindgen option",
                    ),
                )
            })?,
            wit_ns,
            wit_pkg,
            exposed_interface_allow_list: exposed_interface_allow_list.unwrap_or_default().into(),
            exposed_interface_deny_list: exposed_interface_deny_list.unwrap_or_default().into(),
            wit_bindgen_cfg: Some(wit_bindgen_cfg.ok_or_else(|| {
                syn::Error::new(
                    call_site,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing/invalid 'wit_bindgen_cfg' arguments",
                    ),
                )
            })?),
            replace_witified_maps,
        })
    }
}

/// Keywords that are used by this macro
mod keywords {
    syn::custom_keyword!(contract);
    syn::custom_keyword!(wit_namespace);
    syn::custom_keyword!(wit_package);
    syn::custom_keyword!(impl_struct);
    syn::custom_keyword!(wit_bindgen_cfg);
    syn::custom_keyword!(exposed_interface_allow_list);
    syn::custom_keyword!(exposed_interface_deny_list);
    syn::custom_keyword!(replace_witified_maps);
}

/// Options that can be used to perform bindgen
#[allow(clippy::large_enum_variant)]
pub(crate) enum ProviderBindgenConfigOption {
    /// Wasmcloud contract that should be the generated provider
    Contract(LitStr),

    /// Struct that will implement the WIT world
    ImplStruct(Ident),

    /// WIT namespace name
    WitNamespace(LitStr),

    /// WIT package name
    WitPackage(LitStr),

    /// Wit Bindgen configuration (mostly passed on directly to vendored bindgen)
    WitBindgenCfg(WitBindgenConfig),

    /// '<namespace>:<package>/<interface>' combinations that are allowed to be exposed over the lattice
    ///
    /// If no interfaces are specified, all are allowed.
    /// If one or more interfaces are specified, then only those interfaces will be exposed over the lattice.
    ///
    /// If combined with the deny list, this listing will be used first (creating the list of allowed fns).
    ExposedFnAllowList(WitFnList),

    /// '<namespace>:<package>/<interface>' combinations that are explicitly disallowed from being exposed over the lattice.
    ///
    /// If combined with the allow list, this listing will be used last (filtering the list of allowed fns).
    ExposedFnDenyList(WitFnList),

    /// Strategy (e.x. first argument, bundle arguments into struct) to use
    /// when serializing exported WIT interfaces to be sent across the lattice
    ReplaceWitifiedMaps(syn::LitBool),
}

impl Parse for ProviderBindgenConfigOption {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let l = input.lookahead1();
        if l.peek(keywords::contract) {
            input.parse::<keywords::contract>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::Contract(input.parse()?))
        } else if l.peek(keywords::impl_struct) {
            input.parse::<keywords::impl_struct>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::ImplStruct(input.parse()?))
        } else if l.peek(keywords::exposed_interface_allow_list) {
            input.parse::<keywords::exposed_interface_allow_list>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::ExposedFnAllowList(
                input.parse()?,
            ))
        } else if l.peek(keywords::exposed_interface_deny_list) {
            input.parse::<keywords::exposed_interface_deny_list>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::ExposedFnDenyList(
                input.parse()?,
            ))
        } else if l.peek(keywords::wit_bindgen_cfg) {
            input.parse::<keywords::wit_bindgen_cfg>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::WitBindgenCfg(input.parse()?))
        } else if l.peek(keywords::wit_namespace) {
            input.parse::<keywords::wit_namespace>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::WitNamespace(input.parse()?))
        } else if l.peek(keywords::wit_package) {
            input.parse::<keywords::wit_package>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::WitPackage(input.parse()?))
        } else if l.peek(keywords::replace_witified_maps) {
            input.parse::<keywords::replace_witified_maps>()?;
            input.parse::<Token![:]>()?;
            Ok(ProviderBindgenConfigOption::ReplaceWitifiedMaps(
                input.parse()?,
            ))
        } else {
            Err(syn::Error::new(
                Span::call_site(),
                "unrecognized keyword provided to wasmcloud_provider_wit_bindgen",
            ))
        }
    }
}
