use anyhow::{bail, Context};
use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Punct, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, ToTokens, TokenStreamExt};
use syn::parse::Parse;
use syn::punctuated::Punctuated;
use syn::{bracketed, parse_quote, FnArg, ImplItemFn, LitStr, Token};

use tracing::debug;
use wit_parser::{Handle, Result_, Stream, Tuple, TypeDefKind};

use crate::rust::{convert_to_owned_type_arg, ToRustType};
use crate::{
    ExportedLatticeMethod, LatticeExposedInterface, ProviderBindgenConfig, StructLookup, TypeLookup,
};

/// '.' delimited module path to an existing WIT interface (ex. 'wasmcloud:keyvalue/key-value.get')
pub(crate) type WitInterfacePath = String;

pub(crate) type WitPackageName = String;
pub(crate) type WitNamespaceName = String;
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
                    [] => Ok(quote!(())),
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
                        Ok(quote!(( #( #types )* )))
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
        wit_parser::Type::Bool => Ok(quote!(bool)),
        wit_parser::Type::U8 => Ok(quote!(u8)),
        wit_parser::Type::U16 => Ok(quote!(u16)),
        wit_parser::Type::U32 => Ok(quote!(u32)),
        wit_parser::Type::U64 => Ok(quote!(u64)),
        wit_parser::Type::S8 => Ok(quote!(s8)),
        wit_parser::Type::S16 => Ok(quote!(s16)),
        wit_parser::Type::S32 => Ok(quote!(s32)),
        wit_parser::Type::S64 => Ok(quote!(s64)),
        wit_parser::Type::Float32 => Ok(quote!(f32)),
        wit_parser::Type::Float64 => Ok(quote!(f64)),
        wit_parser::Type::Char => Ok(quote!(char)),
        wit_parser::Type::String => Ok(quote!(String)),
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
            Ok(quote!(Option<#ty>))
        }
        // Nested type case (Result<...>)
        TypeDefKind::Result(Result_ { ok, err }) => {
            let ok_ty = if let Some(ty_id) = ok {
                convert_wit_type(ty_id, cfg)?
            } else {
                quote!(())
            };
            let err_ty = if let Some(ty_id) = err {
                convert_wit_type(ty_id, cfg)?
            } else {
                quote!(())
            };
            Ok(quote!(Result<#ok_ty, #err_ty>))
        }
        // Nested type case (List<...>)
        TypeDefKind::List(ty_id) => {
            let ty = convert_wit_type(ty_id, cfg)?;
            Ok(quote!(Vec<#ty>))
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
                tuple_types.append_all(quote!(#tokens));
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
            Ok(quote!(impl Stream<Item=#element_ty>))
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

/// Translate a wit-bindgen generated trait function for use across the lattice
pub(crate) fn translate_export_fn_for_lattice(
    bindgen_cfg: &ProviderBindgenConfig,
    operation_name: LitStr,
    trait_method: &ImplItemFn,
    struct_lookup: &StructLookup,
    type_lookup: &TypeLookup,
) -> anyhow::Result<ExportedLatticeMethod> {
    // If there are no arguments, then we can add a lattice method with nothing
    if trait_method.sig.inputs.is_empty() {
        return Ok(ExportedLatticeMethod {
            operation_name,
            func_name: trait_method.sig.ident.clone(),
            invocation_args: Vec::new(),
            invocation_return: trait_method.sig.output.clone(),
        });
    }

    // Transform the arguments and remove any lifetimes by manually converting references to owned data
    // (i.e. doing things like converting a type like &str to String mechanically)
    let invocation_args = trait_method
        .sig
        // Get all function inputs for the function signature
        .inputs
        .iter()
        .map(|arg| {
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

            // Re-parse the owned type (which may have changed) into a FnArg pull out just the type
            let (_, ty) = process_fn_arg(&syn::parse2::<FnArg>(owned_type_tokens)?)
                .context("failed to process fn arg for type")?;

            // Add the invocation argument name to the list,
            // so that when we convert this `ExportedLatticeMethod` into an exported function
            // we can re-create the arguments as if they were never bundled into a struct.
            Ok((arg_name, ty))
        })
        .collect::<anyhow::Result<Vec<(Ident, TokenStream)>>>()
        .context("failed to collect fn args")?;

    Ok(ExportedLatticeMethod {
        operation_name,
        func_name: trait_method.sig.ident.clone(),
        invocation_args,
        invocation_return: trait_method.sig.output.clone(),
    })
}

/// Translate an imported WIT function automatically by detecting the number of arguments,
/// producing a list of struct definitions ([`TokenStream`]s) that need to be generated/created
/// and a list of function (method) definitions ([`TokenStream`]s) that need to be hung off of the
/// implementing struct for the given interface.
///
/// Note that the lists do not necessarily match, for example an interface with only one method with no arguments,
/// there is no extra structs that need to be made, but there is one function (the method) that needs to be crafted.
pub(crate) fn translate_import_fn_for_lattice(
    iface: &wit_parser::Interface,
    iface_fn_name: &String,
    iface_fn: &wit_parser::Function,
    cfg: &ProviderBindgenConfig,
) -> anyhow::Result<TokenStream> {
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

    // Convert the WIT result type into a Rust type
    let result_rust_type = if iface_fn.params.is_empty() {
        quote!(())
    } else {
        iface_fn.results.to_rust_type(cfg).with_context(|| {
            format!(
                "Failed to convert WIT function results (returns) while parsing interface [{}]",
                iface.name.clone().unwrap_or("<unknown>".into()),
            )
        })?
    };

    // Build list of params
    let mut param_tokens = Vec::<TokenStream>::new();
    let mut dynamic_tuple_args = Vec::<TokenStream>::new();
    for (name, ty_id) in iface_fn.params.iter() {
        let raw_type = convert_wit_type(ty_id, cfg)?;
        let name = format_ident!("{}", name);
        param_tokens.push(quote!(#name: #raw_type));
        dynamic_tuple_args.push(name.to_token_stream());
    }

    // Generate the tokens required for the function
    let func_ts = quote!(
        async fn #fn_name(
            &self,
            #( #param_tokens ),*
        ) -> ::wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::error::InvocationResult<#result_rust_type> {
            use ::wasmcloud_provider_wit_bindgen::deps::wrpc_transport::Client;

            let (result, tx) = match self.wrpc_client
                .invoke_static::<#result_rust_type>(#instance_lit_str, #fn_name_lit_str, (#( #dynamic_tuple_args ),*))
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

    Ok(func_ts)
}

/// Process a fn argument to retreive the argument name and type name used
pub(crate) fn process_fn_arg(arg: &FnArg) -> anyhow::Result<(Ident, TokenStream)> {
    // Retrieve the type pattern ascription (i.e. 'arg: Type') out of the first arg
    let pat_type = if let syn::FnArg::Typed(pt) = arg {
        pt
    } else {
        bail!("failed to parse pat type out of ");
    };

    // Retrieve argument name
    let mut arg_name = if let syn::Pat::Ident(n) = pat_type.pat.as_ref() {
        n.ident.clone()
    } else {
        bail!("unexpectedly non-ident pattern in {pat_type:#?}");
    };

    // If the argument name ends in _map, and the type matches a witified map (i.e. list<tuple<T, T>>)
    // then convert the type into a map *before* using it
    let type_name = match (
        arg_name.to_string().ends_with("_map"),
        extract_witified_map(
            &pat_type
                .ty
                .as_ref()
                .to_token_stream()
                .into_iter()
                .collect::<Vec<TokenTree>>(),
        ),
    ) {
        (true, Some(map_type)) => {
            arg_name = Ident::new(
                arg_name.to_string().trim_end_matches("_map"),
                arg_name.span(),
            );
            quote!(#map_type)
        }
        _ => pat_type.ty.as_ref().to_token_stream(),
    };

    Ok((arg_name, type_name))
}
