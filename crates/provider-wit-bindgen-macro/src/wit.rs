use std::str::FromStr;

use anyhow::{bail, ensure, Context};
use heck::{ToKebabCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Punct, Span, TokenStream, TokenTree};
use quote::{format_ident, ToTokens, TokenStreamExt};
use syn::parse::Parse;
use syn::punctuated::Punctuated;
use syn::{bracketed, parse_quote, ImplItemFn, LitStr, Token};

use tracing::debug;
use wit_parser::{Handle, Result_, Stream, Tuple, TypeDefKind};

use crate::rust::{convert_to_owned_type_arg, ToRustType};
use crate::{
    process_fn_arg, LatticeExposedInterface, LatticeMethod, ProviderBindgenConfig, StructLookup,
    TypeLookup,
};

/// Helper to differentiate token stream contents
type FunctionTokenStream = TokenStream;

/// Helper to differentiate token stream contents
type StructTokenStream = TokenStream;

pub(crate) type WitNamespaceName = String;
pub(crate) type WitPackageName = String;
/// '.' delimited module path to an existing WIT interface (ex. 'wasmcloud.keyvalue.key_value')
pub(crate) type WitInterfacePath = String;
pub(crate) type WitFunctionName = String;

/// Wrapper for a list of qualified WIT function names
#[derive(Debug, Default)]
pub(crate) struct WitFnList {
    inner: Vec<LatticeExposedInterface>,
}

impl From<WitFnList> for Vec<LatticeExposedInterface> {
    fn from(value: WitFnList) -> Self {
        value.inner
    }
}

impl Parse for WitFnList {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut inner = Vec::new();
        let names;
        bracketed!(names in input);
        let fns = Punctuated::<LitStr, Token![,]>::parse_terminated(&names)?;
        for name_ident in fns {
            let name = name_ident.value();
            let mut split = name.split(':');
            let ns = split.next();
            let package_iface = split.next().and_then(|rhs| rhs.split_once('/'));
            match (ns, package_iface) {
                (Some(ns), Some((pkg, fn_name))) => {
                    debug!("successfully parsed interface {ns}:{pkg}/{fn_name}");
                    inner.push((ns.into(), pkg.into(), fn_name.into()));
                }
                _ => {
                    return syn::Result::Err(
                        syn::Error::new(
                            Span::call_site(),
                            format!("allow/deny list entries must be of the form \"<ns>:<package>/<interface>\", failed to process [\"{name}\"]")
                        )
                    );
                }
            }
        }
        Ok(Self { inner })
    }
}

impl ToRustType for wit_parser::Results {
    fn to_rust_type(&self, cfg: &ProviderBindgenConfig) -> anyhow::Result<TokenStream> {
        match self {
            // Convert a named return (usually simple, either empty or tuple)
            wit_parser::Results::Named(params) => {
                match params[..] {
                    // No results
                    // i.e. `func(arg: string)`
                    [] => Ok(quote::quote!(())),
                    // One or more results:
                    //
                    // e.x. `func(arg: string) -> string`
                    // e.x. `func(arg: string) -> ( string, string )`
                    ref params => {
                        let mut types: Vec<Ident> = Vec::new();
                        for (_, ty) in params.iter() {
                            types.push(syn::parse2::<Ident>(
                                convert_wit_type(ty, cfg).with_context(|| {
                                    format!("failed to convert WIT type [{ty:?}] to rust type]")
                                })?,
                            )?);
                        }
                        Ok(quote::quote!(( #( #types )* )))
                    }
                }
            }
            // Convert a return with arbitrary complexity
            wit_parser::Results::Anon(t) => {
                convert_wit_type(t, cfg).context("failed to find anonymous WIT type")
            }
        }
    }
}

/// Convert a WIT type into a TokenStream that contains a Rust type
///
/// This function is co-recursive with `convert_wit_typedef`, since type defs
/// and types can be recursively intertwined
pub(crate) fn convert_wit_type(
    t: &wit_parser::Type,
    cfg: &ProviderBindgenConfig,
) -> anyhow::Result<TokenStream> {
    match t {
        wit_parser::Type::Bool => Ok(quote::quote!(bool)),
        wit_parser::Type::U8 => Ok(quote::quote!(u8)),
        wit_parser::Type::U16 => Ok(quote::quote!(u16)),
        wit_parser::Type::U32 => Ok(quote::quote!(u32)),
        wit_parser::Type::U64 => Ok(quote::quote!(u64)),
        wit_parser::Type::S8 => Ok(quote::quote!(s8)),
        wit_parser::Type::S16 => Ok(quote::quote!(s16)),
        wit_parser::Type::S32 => Ok(quote::quote!(s32)),
        wit_parser::Type::S64 => Ok(quote::quote!(s64)),
        wit_parser::Type::Float32 => Ok(quote::quote!(f32)),
        wit_parser::Type::Float64 => Ok(quote::quote!(f64)),
        wit_parser::Type::Char => Ok(quote::quote!(char)),
        wit_parser::Type::String => Ok(quote::quote!(String)),
        wit_parser::Type::Id(tydef) => {
            // Look up the type in the WIT resolver
            let type_def = &cfg
                .wit_bindgen_cfg
                .as_ref()
                .context("WIT bindgen config missing")?
                .resolve
                .types[*tydef];

            convert_wit_typedef(type_def, cfg)
        }
    }
}

/// Convert a [`wit_parser::TypeDef`] (which can be found inside a [`wit_parser::Type`])
/// into a [`TokenStream`] which corresponds to a Rust type
///
/// This function is co-recursive with `convert_wit_type`, since type defs
/// and types can be recursively intertwined
pub(crate) fn convert_wit_typedef(
    type_def: &wit_parser::TypeDef,
    cfg: &ProviderBindgenConfig,
) -> anyhow::Result<TokenStream> {
    // For nested types, the type_def.name is None and the kind goes deeper
    match &type_def.kind {
        // Nested type case (Option<...>)
        TypeDefKind::Option(ty_id) => {
            let ty = convert_wit_type(ty_id, cfg)?;
            Ok(quote::quote!(Option<#ty>))
        }
        // Nested type case (Result<...>)
        TypeDefKind::Result(Result_ { ok, err }) => {
            let ok_ty = if let Some(ty_id) = ok {
                convert_wit_type(ty_id, cfg)?
            } else {
                quote::quote!(())
            };
            let err_ty = if let Some(ty_id) = err {
                convert_wit_type(ty_id, cfg)?
            } else {
                quote::quote!(())
            };
            Ok(quote::quote!(Result<#ok_ty, #err_ty>))
        }
        // Nested type case (List<...>)
        TypeDefKind::List(ty_id) => {
            let ty = convert_wit_type(ty_id, cfg)?;
            Ok(quote::quote!(Vec<#ty>))
        }
        // Nested type case (owned data)
        TypeDefKind::Handle(Handle::Own(ty_idx)) => {
            let ty_def = cfg
                .wit_bindgen_cfg
                .as_ref()
                .context("missing WIT bindgen cfg resolver")?
                .resolve
                .types
                .get(*ty_idx)
                .context("failed to find type with given ID in WIT resolver")?;
            convert_wit_typedef(ty_def, cfg)
        }
        // Nested type case (borrowed data)
        TypeDefKind::Handle(Handle::Borrow(ty_idx)) => {
            let ty_def = cfg
                .wit_bindgen_cfg
                .as_ref()
                .context("missing WIT bindgen cfg resolver")?
                .resolve
                .types
                .get(*ty_idx)
                .context("failed to find type with given ID in WIT resolver")?;
            convert_wit_typedef(ty_def, cfg)
        }
        // Nested type case (Tuple)
        TypeDefKind::Tuple(Tuple { types }) => {
            let mut tuple_types = TokenStream::new();
            for (idx, tokens) in types
                .iter()
                .map(|t| convert_wit_type(t, cfg))
                .collect::<anyhow::Result<Vec<TokenStream>>>()
                .context("failed to parse all types in Tuple w/ types {types:?}")?
                .iter()
                .enumerate()
            {
                tuple_types.append_all(quote::quote!(#tokens));
                if idx != types.len() - 1 {
                    tuple_types.append(TokenTree::Punct(Punct::new(
                        ',',
                        proc_macro2::Spacing::Alone,
                    )));
                }
            }
            Ok(
                proc_macro2::Group::new(proc_macro2::Delimiter::Parenthesis, tuple_types)
                    .to_token_stream(),
            )
        }
        TypeDefKind::Stream(Stream { element, .. }) => {
            let element_ty = convert_wit_type(&element.context("missing type for stream")?, cfg)?;
            Ok(quote::quote!(impl Stream<Item=#element_ty>))
        }
        // Nested types that come through can run through
        // there's a potential for a cycle here, but it's unlikely
        TypeDefKind::Type(t) => convert_wit_type(t, cfg),
        // In the straight-forward cases below, we must just use the
        // name of the type and hope for the best:
        //
        // Since we get the wit type name here (in kebab case)
        // we'll expect the custom oxidized type to be upper camel case
        // (e.x. `chunk` -> `Chunk`)
        TypeDefKind::Variant(_) | TypeDefKind::Resource | TypeDefKind::Unknown => type_def
            .name
            .as_ref()
            .map(|v| v.to_upper_camel_case())
            .map(|v| Ident::new(&v, Span::call_site()).to_token_stream())
            .with_context(|| format!("failed to parse wit type def for type_def: {type_def:?}")),

        // For records that we encounter, they will be translated to Rust Structs by bindgen
        // we can pretend the struct exists because by the time the macro is done, it will.
        TypeDefKind::Record(_) => {
            let struct_name = format_ident!(
                "{}",
                type_def
                    .name
                    .as_ref()
                    .context("unexpectedly missing name for typedef")?
                    .to_upper_camel_case()
            );
            Ok(struct_name.to_token_stream())
        }

        _ => bail!("unsupported type kind {type_def:#?}"),
    }
}

/// Attempt to extract key and value types from a tree of tokens that is a witified map
///
/// For example, the following Rust type submitted as a list of tokens would be parsed successfully:
///
/// ```rust,ignore
/// Vec<(String, String)>
/// ```
pub(crate) fn extract_witified_map(input: &[TokenTree]) -> Option<TokenStream> {
    match input {
        // Handle WIT-ified maps that are wrapped in Option or Vec
        // i.e. Option<Vec<(K, V)>> or Vec<Vec<(K, V)>>
        [
            container @ TokenTree::Ident(container_ident), // Option
            TokenTree::Punct(p1), // <
            inner @ .. ,
            TokenTree::Punct(p2), // >
        ] if p1.as_char() == '<'
            && p2.as_char() == '>'
            && (*container_ident == "Option" || *container_ident == "Vec")
            // We need to know that the inner type is *not* a group
            // since this branch is only meant to a list of tuples (Vec<Vec<(....)>>)
            //
            // If the inner tokens are a group then we should head to the final case branch instead
            && inner.first().is_some_and(|tokens| !matches!(tokens, TokenTree::Group { .. })) => {
            let container_ts = container.to_token_stream();
            // Recursive call to extract the witified map from the inner type, re-wrapping in the container
            extract_witified_map(inner)
                .map(|t| parse_quote!(#container_ts<#t>))
        },

        // Handle WIT-ified maps that are unwrapped (i.e. a Vec<(K, V)>)
        [
            TokenTree::Ident(vec_ty),
            TokenTree::Punct(p1), // <
            TokenTree::Group(g),
            TokenTree::Punct(p2), // >
        ] if *vec_ty == "Vec" && p1.to_string() == "<" && p2.to_string() == ">" => {
            // The delimeter to the internal group must be delimited by parenthesis (it's a tuple)
            if g.delimiter() != proc_macro2::Delimiter::Parenthesis {
                return None;
            }

            // Now that we have the internal group (Vec< ...this bit... >),
            // we can extract the key and value type for the vec as a tuple
            let tokens = g.stream().into_iter().collect::<Vec<TokenTree>>();

            // Find the index of the comma which splits the types
            let comma_idx = tokens.iter().position(|t| matches!(t, TokenTree::Punct(p) if p.to_string() == ","))?;

            let key_type = TokenStream::from_iter(tokens[0..comma_idx].to_owned());
            let value_type = TokenStream::from_iter(tokens[comma_idx + 1..].to_owned());
            let map_type = parse_quote!(::std::collections::HashMap<#key_type,#value_type>);
            Some(map_type)
        },

        // All other matches cannot be WIT-ified maps
        _ => None,
    }
}

/// The strategy used to expose a function and its arguments on the wasmCloud lattice
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub(crate) enum WitFunctionLatticeTranslationStrategy {
    /// Automatically determine how to bundle a function and its arguments to be sent over the lattice
    ///
    /// Generally, this means detecting whether the function has a single argument or multiple
    /// and choosing whether to send that single argument or bundle up all arguments into a generated Invocation struct.
    #[default]
    Auto,

    /// First argument usage assumes that every function that should be exported to the lattice
    /// has *one* argument, which is the object that should be serialized and set out onto the lattice.
    ///
    /// For example, the following WIT:
    ///
    /// ```ignore
    /// interface example {
    ///   f: func(input: string);
    ///
    ///   record g-request {
    ///     first: string;
    ///     second: u32;
    ///     third: bool;
    ///   }
    ///
    ///   g: func(input: g-request);
    ///
    ///   h: func(first: bool, second: string);
    /// }
    /// ```
    ///
    /// Under this setting, bindgen will produce an error on function `h`, as it contains multiple arguments
    FirstArgument,

    /// Argument bundling assumes that every function that should be exported to the lattice
    /// has *one or more* arguments, which are bundled into an object that should be serialized and set out onto the lattice.
    ///
    /// For example, the following WIT:
    ///
    /// ```ignore
    /// package examples:arg-bundle;
    ///
    /// interface example {
    ///   f: func(input: string);
    ///
    ///   record g-request {
    ///     first: string;
    ///     second: u32;
    ///     third: bool;
    ///   }
    ///
    ///   g: func(input: g-request);
    ///
    ///   h: func(first: bool, second: string);
    /// }
    /// ```
    ///
    /// Under this setting, bindgen will not produce an error on any function, but will be slightly
    /// inefficient as it will wrap `f` in a generated `ExamplesArgBundleFInvocation` struct (containing one member, `input`).
    BundleArguments,
}

impl WitFunctionLatticeTranslationStrategy {
    /// Translate a wit-bindgen generated trait function for use across the lattice
    pub(crate) fn translate_export_fn_for_lattice(
        &self,
        bindgen_cfg: &ProviderBindgenConfig,
        wit_iface_path: WitInterfacePath,
        trait_method: &ImplItemFn,
        struct_lookup: &StructLookup,
        type_lookup: &TypeLookup,
    ) -> anyhow::Result<(WitInterfacePath, LatticeMethod)> {
        // Rebuild the fully-qualified WIT operation name
        let wit_operation = match wit_iface_path.split('.').collect::<Vec<&str>>()[..] {
            [wit_ns, wit_pkg, iface] => {
                format!(
                    "{}:{}/{}.{}",
                    wit_ns.to_kebab_case(),
                    wit_pkg.to_kebab_case(),
                    iface.to_kebab_case(),
                    trait_method.sig.ident.to_string().to_kebab_case()
                )
            }
            _ => bail!("unexpected interface path, expected 3 components"),
        };
        let lattice_method_name = LitStr::new(&wit_operation, trait_method.sig.ident.span());

        // Convert the iface path into an upper camel case representation, for future conversions to use
        let wit_iface_upper_camel = wit_iface_path
            .split('.')
            .map(|v| v.to_upper_camel_case())
            .collect::<String>();

        match self {
            WitFunctionLatticeTranslationStrategy::Auto => match trait_method.sig.inputs.len() {
                0 | 1 => Self::translate_export_fn_via_first_arg(
                    wit_iface_upper_camel,
                    lattice_method_name,
                    trait_method,
                    struct_lookup,
                    type_lookup,
                ),
                _ => Self::translate_export_fn_via_bundled_args(
                    bindgen_cfg,
                    wit_iface_upper_camel,
                    lattice_method_name,
                    trait_method,
                    struct_lookup,
                    type_lookup,
                ),
            },
            WitFunctionLatticeTranslationStrategy::FirstArgument => {
                Self::translate_export_fn_via_first_arg(
                    wit_iface_upper_camel,
                    lattice_method_name,
                    trait_method,
                    struct_lookup,
                    type_lookup,
                )
            }
            WitFunctionLatticeTranslationStrategy::BundleArguments => {
                Self::translate_export_fn_via_bundled_args(
                    bindgen_cfg,
                    wit_iface_upper_camel,
                    lattice_method_name,
                    trait_method,
                    struct_lookup,
                    type_lookup,
                )
            }
        }
    }

    /// Translate a function for use on the lattice via the first argument.
    /// Functions that cannot be translated properly via this method will fail.
    pub(crate) fn translate_export_fn_via_first_arg(
        wit_iface_upper_camel: String,
        lattice_method_name: LitStr,
        trait_method: &ImplItemFn,
        _struct_lookup: &StructLookup,
        _type_lookup: &TypeLookup,
    ) -> anyhow::Result<(WitInterfacePath, LatticeMethod)> {
        // It is possible to force first argument style handling, so double check
        ensure!(
            trait_method.sig.inputs.len() <= 1,
            "forcing translation of first arg for trait method [{}] that has more than one arg",
            trait_method.sig.ident,
        );

        // If there are no arguments, then we can add a lattice method with nothing:
        if trait_method.sig.inputs.is_empty() {
            return Ok((
                wit_iface_upper_camel,
                LatticeMethod {
                    lattice_method_name,
                    type_name: None,
                    func_name: trait_method.sig.ident.clone(),
                    struct_members: None,
                    invocation_args: Vec::new(),
                    invocation_return: trait_method.sig.output.clone(),
                },
            ));
        }

        // Get the first function argument, which will become the type sent across the lattice
        // Get the remaining tokens after the argument name and colon type name from the first argument
        let first_arg = trait_method.sig.inputs.iter().next().context(format!(
            "trait method [{}] has no arguments yet is attempting to translate via first arg",
            trait_method.sig.ident,
        ))?;

        // Process a function argument to retrieve the argument name and type name
        let (arg_name, type_name) = process_fn_arg(first_arg)?;

        Ok((
            wit_iface_upper_camel,
            LatticeMethod {
                lattice_method_name,
                type_name: Some(type_name.clone()),
                func_name: trait_method.sig.ident.clone(),
                struct_members: None,
                invocation_args: vec![(arg_name, type_name)],
                invocation_return: trait_method.sig.output.clone(),
            },
        ))
    }

    /// Translate a function for use on the lattice via bundled args.
    /// Functions that cannot be translated properly via this method will fail.
    pub(crate) fn translate_export_fn_via_bundled_args(
        bindgen_cfg: &ProviderBindgenConfig,
        wit_iface_upper_camel: String,
        lattice_method_name: LitStr,
        trait_method: &ImplItemFn,
        struct_lookup: &StructLookup,
        type_lookup: &TypeLookup,
    ) -> anyhow::Result<(WitInterfacePath, LatticeMethod)> {
        // Create an identifier for the new struct that will represent the function invocation coming
        // across the lattice, in a <CamelCaseModule><CamelCaseInterface><CamelCaseFunctionName> pattern
        // (ex. MessagingConsumerRequestMultiInvocation)
        let struct_name = format_ident!(
            "{}{}Invocation",
            wit_iface_upper_camel,
            trait_method.sig.ident.to_string().to_upper_camel_case()
        );

        // Build a list of invocation arguments & their types
        let mut invocation_args: Vec<(Ident, TokenStream)> = Vec::new();

        // Transform the members and remove any lifetimes by manually converting references to owned data
        // (i.e. doing things like converting a type like &str to String mechanically)
        let struct_members = trait_method
            .sig
            // Get all function inputs for the function signature
            .inputs
            .iter()
            .enumerate()
            .fold(TokenStream::new(), |mut tokens, (idx, arg)| {
                // If we're not the first index, add a comma
                if idx != 0 {
                    tokens.append_all([&TokenTree::Punct(Punct::new(
                        ',',
                        proc_macro2::Spacing::Alone,
                    ))]);
                }

                // For the current input argument in the function signature,
                // convert known types to ones that can be used as invocation struct members.
                //
                // i.e. given some `record type {...}` defined in WIT, a Rust `struct Type {...}` will be produced.
                // if we see some::path::to::Type, we should replace it with Type, because all of those types have been
                // extracted, raised and put at the top level by our bindgen
                let (arg_name, owned_type_tokens) = convert_to_owned_type_arg(
                    struct_lookup,
                    type_lookup,
                    arg,
                    bindgen_cfg.replace_witified_maps,
                );

                // Add the invocation argument name to the list,
                // so that when we convert this LatticeMethod into an exported function
                // we can re-create the arguments as if they were never bundled into a struct.
                invocation_args.push((arg_name, owned_type_tokens.clone()));

                // Add the generated `FnArg` tokens
                tokens.extend(owned_type_tokens);

                tokens
            });

        Ok((
            wit_iface_upper_camel,
            LatticeMethod {
                lattice_method_name,
                type_name: Some(struct_name.to_token_stream()),
                struct_members: Some(struct_members),
                func_name: trait_method.sig.ident.clone(),
                invocation_args,
                invocation_return: trait_method.sig.output.clone(),
            },
        ))
    }

    /// Translate an exported WIT function automatically by detecting the number of arguments
    pub(crate) fn translate_import_fn_for_lattice(
        &self,
        iface: &wit_parser::Interface,
        iface_fn_name: &String,
        iface_fn: &wit_parser::Function,
        cfg: &ProviderBindgenConfig,
    ) -> anyhow::Result<(Vec<StructTokenStream>, Vec<FunctionTokenStream>)> {
        match self {
            WitFunctionLatticeTranslationStrategy::Auto => {
                match &iface_fn.params.as_slice() {
                    // Handle the no-parameter case
                    [] => {
                        let fn_name =
                            Ident::new(iface_fn_name.to_snake_case().as_str(), Span::call_site());
                        // Derive the WIT instance (<ns>:<pkg>/<iface>) & fn name
                        let iface_name = iface
                            .name
                            .clone()
                            .context("unexpectedly missing iface name")?;
                        let instance_lit_str = LitStr::new(
                            format!("{}/{iface_name}", cfg.contract).as_str(),
                            Span::call_site(),
                        );
                        let fn_name_lit_str = LitStr::new(iface_fn_name, Span::call_site());

                        let func_ts = quote::quote!(
                            async fn #fn_name(
                                &self,
                            ) -> ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationResult<()> {
                                use ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport::Client;

                                // Invoke the other end
                                let (results, tx) = match self.wrpc_client
                                    .invoke_static::<()>(#instance_lit_str, #fn_name_lit_str, ())
                                    .await {
                                        Ok(v) => v,
                                        Err(e) => {
                                            return Err(
                                                ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationError::Unexpected(
                                                    format!("invoke for operation [{}.{}] failed: {e}", #instance_lit_str, #fn_name_lit_str)
                                                )
                                            )
                                        }
                                    };
                                // Wait for params to send
                                match tx.await {
                                    Ok(_) => {},
                                    Err(e) => {
                                        return Err(
                                            ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationError::Unexpected(
                                                format!("failed to send params: {e}")
                                            )
                                        )
                                    },
                                };

                                Ok(())
                            }
                        );

                        Ok((vec![], vec![func_ts]))
                    }
                    // Handle the single parameter case
                    [(arg_name, arg_type)] => {
                        // If there is one input, we can use it assuming it is the message being sent out onto the lattice
                        Self::translate_import_fn_via_first_arg(
                            iface,
                            iface_fn_name,
                            arg_name,
                            arg_type,
                            &iface_fn.results,
                            cfg,
                        )
                    }
                    // For exported functions with >1 parameters, we must attempt bundling the arguments into one object
                    // to be sent out over the lattice
                    _ => Self::translate_import_fn_via_bundled_args(
                        iface,
                        iface_fn_name,
                        iface_fn,
                        cfg,
                    ),
                }
            }
            WitFunctionLatticeTranslationStrategy::FirstArgument => {
                if let [(arg_name, arg_type)] = &iface_fn.params.as_slice() {
                    Self::translate_import_fn_via_first_arg(
                        iface,
                        iface_fn_name,
                        arg_name,
                        arg_type,
                        &iface_fn.results,
                        cfg,
                    )
                } else {
                    bail!("function parameters for interface function {iface_fn_name} have more than one argument");
                }
            }
            WitFunctionLatticeTranslationStrategy::BundleArguments => {
                Self::translate_import_fn_via_bundled_args(iface, iface_fn_name, iface_fn, cfg)
            }
        }
    }

    /// Translate an exported WIT function via first argument
    pub(crate) fn translate_import_fn_via_first_arg(
        iface: &wit_parser::Interface,
        iface_fn_name: &str,
        arg_name: &str,
        arg_type: &wit_parser::Type,
        results: &wit_parser::Results,
        cfg: &ProviderBindgenConfig,
    ) -> anyhow::Result<(Vec<StructTokenStream>, Vec<FunctionTokenStream>)> {
        let rust_type = convert_wit_type(arg_type, cfg)?;
        let fn_name = Ident::new(iface_fn_name.to_snake_case().as_str(), Span::call_site());
        let arg_name_ident = Ident::new(arg_name, Span::call_site());
        let iface_name = iface
            .name
            .clone()
            .context("unexpectedly missing iface name")?;

        // Derive the WIT instance (<ns>:<pkg>/<iface>) & fn name
        let instance_lit_str = LitStr::new(
            format!("{}/{iface_name}", cfg.contract).as_str(),
            Span::call_site(),
        );
        let fn_name_lit_str = LitStr::new(iface_fn_name, Span::call_site());

        // Convert the WIT result type into a Rust type
        let result_rust_type = results.to_rust_type(cfg).with_context(|| {
            format!(
                "Failed to convert WIT function results (returns) while parsing interface [{}]",
                iface.name.clone().unwrap_or("<unknown>".into()),
            )
        })?;

        // Return the generated function with appropriate args & return
        let func_tokens = quote::quote!(
            async fn #fn_name(
                &self,
                #arg_name_ident: #rust_type
            ) -> ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationResult<#result_rust_type> {
                use ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport::{EncodeSync, Receive};
                use ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport::Client;

                // Invoke the other end
                let (result, tx) = match self.wrpc_client
                    .invoke_static::<#result_rust_type>(#instance_lit_str, #fn_name_lit_str, #arg_name_ident)
                    .await {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(
                                ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationError::Unexpected(
                                    format!("invoke for operation [{}.{}] failed: {e}", #instance_lit_str, #fn_name_lit_str)
                                )
                            );
                        }
                    };

                // Wait for params to send
                match tx.await {
                    Ok(_) => {},
                    Err(e) => {
                        return Err(
                            ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationError::Unexpected(
                                format!("failed to send params: {e}")
                            )
                        )
                    },
                };

                Ok(result)
            }
        );

        Ok((vec![], vec![func_tokens]))
    }

    /// Translate an exported WIT function via bundled arguments
    pub(crate) fn translate_import_fn_via_bundled_args(
        iface: &wit_parser::Interface,
        iface_fn_name: &str,
        iface_fn: &wit_parser::Function,
        cfg: &ProviderBindgenConfig,
    ) -> anyhow::Result<(Vec<StructTokenStream>, Vec<FunctionTokenStream>)> {
        let fn_params = &iface_fn.params;
        let fn_results = &iface_fn.results;
        let fn_name = Ident::new(iface_fn_name.to_snake_case().as_str(), Span::call_site());

        // Derive the WIT instance (<ns>:<pkg>/<iface>) & fn name
        let iface_name = iface
            .name
            .clone()
            .context("unexpectedly missing iface name")?;
        let instance_lit_str = LitStr::new(
            format!("{}/{iface_name}", cfg.contract).as_str(),
            Span::call_site(),
        );
        let fn_name_lit_str = LitStr::new(iface_fn_name, Span::call_site());

        // Build the invocation struct that will be used
        let invocation_struct_name = format_ident!("{}Args", iface_fn_name.to_upper_camel_case());

        // Build an Args struct for the arguments to this interface function
        let mut struct_member_tokens: TokenStream = TokenStream::new();
        for (idx, (name, ty_id)) in fn_params.iter().enumerate() {
            let raw_type = convert_wit_type(ty_id, cfg)?;
            let name = format_ident!("{}", name);
            struct_member_tokens.append_all(quote::quote!(#name: #raw_type));
            if idx != fn_params.len() - 1 {
                struct_member_tokens.append(TokenTree::Punct(Punct::new(
                    ',',
                    proc_macro2::Spacing::Alone,
                )));
            }
        }

        // Build a struct that will be used to send args across the lattice
        //
        // This struct will eventually be written out, before the InvocationHandlers
        let invocation_struct_tokens = quote::quote!(
            #[derive(Debug, ::wasmcloud_provider_wit_bindgen::deps::serde::Serialize, ::wasmcloud_provider_wit_bindgen::deps::serde::Deserialize)]
            #[serde(crate = "::wasmcloud_provider_wit_bindgen::deps::serde")]
            pub struct #invocation_struct_name {
                #struct_member_tokens
            }
        );

        // Convert the WIT result type into a Rust type
        let result_rust_type = fn_results.to_rust_type(cfg).with_context(|| {
            format!(
                "Failed to convert WIT function results (returns) while parsing interface [{}]",
                iface.name.clone().unwrap_or("<unknown>".into()),
            )
        })?;

        // Build token stream for the invocation function that can be called
        //
        // This function will eventually be written into the impl of an InvocationHandler
        let func_tokens = quote::quote!(
            async fn #fn_name(
                &self,
                args: #invocation_struct_name,
            ) -> ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationResult<#result_rust_type> {
                use ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport::{EncodeSync, Receive};
                use ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport::Client;

                // Invoke the other end
                let (result, tx) = match self.wrpc_client
                    .invoke_static::<#result_rust_type>(#instance_lit_str, #fn_name_lit_str, DynamicTuple(#struct_member_tokens))
                    .await {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(
                                ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationError::Unexpected(
                                    format!("invoke for operation [{}.{}] failed: {e}", #instance_lit_str, #fn_name_lit_str)
                                )
                            )
                        }
                    };

                // Wait for params to send
                match tx.await {
                    Ok(_) => {},
                    Err(e) => {
                        return Err(
                            ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationError::Unexpected(
                                format!("failed to send params: {e}")
                            )
                        )
                    },
                };

                Ok(result)
            }
        );

        Ok((vec![invocation_struct_tokens], vec![func_tokens]))
    }
}

impl FromStr for WitFunctionLatticeTranslationStrategy {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "bundle-arguments" => Ok(Self::BundleArguments),
            "first-argument" => Ok(Self::FirstArgument),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "failed",
            )),
        }
    }
}

impl Parse for WitFunctionLatticeTranslationStrategy {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let key = input.parse::<LitStr>()?;
        Self::from_str(key.value().as_str())
            .map_err(|e| syn::Error::new::<std::io::Error>(Span::call_site(), e))
    }
}
