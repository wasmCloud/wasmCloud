//! Utilities for working with Rust types

use proc_macro2::{Delimiter, Ident, Punct, Spacing, Span, TokenStream, TokenTree};
use quote::{ToTokens, TokenStreamExt};
use syn::{FnArg, Type};

use crate::wit::extract_witified_map;
use crate::{ProviderBindgenConfig, StructLookup, TypeLookup};

/// A trait that represents things that can be converted to a Rust type
pub(crate) trait ToRustType {
    /// Convert to a Rust type
    fn to_rust_type(&self, cfg: &ProviderBindgenConfig) -> anyhow::Result<TokenStream>;
}

/// Count the number of preceeding "super" calls a given [`syn::Type`] has,
/// if it is a [`syn::Type::Path`]
pub(crate) fn count_preceeding_supers(t: &Type) -> usize {
    if let Type::Path(t) = t {
        t.path
            .segments
            .iter()
            .filter(|s| s.ident == "super")
            .count()
    } else {
        0
    }
}

/// Check if a given TokenStream is the Rust unit type
pub(crate) fn is_rust_unit_type(t: &TokenStream) -> bool {
    t.to_string() == "()"
}

/// Convert a possibly not owned `FnArg` type (ex. `s: &str`) to a TokenStream
/// that represents an owned type `FnArg` (ex. `s: String`)
pub(crate) fn convert_to_owned_type_arg(
    struct_lookup: &StructLookup,
    type_lookup: &TypeLookup,
    arg: &FnArg,
    replace_witified_maps: bool,
) -> (Ident, TokenStream) {
    let arg_name: Ident;
    let mut tokens = TokenStream::new();

    match &arg
        .to_token_stream()
        .into_iter()
        .collect::<Vec<TokenTree>>()[..]
    {
        // pattern: 'name: &T'
        simple_ref @ [
            TokenTree::Ident(ref n), // name
            TokenTree::Punct(_), // :
            TokenTree::Punct(ref p), // &
            TokenTree::Ident(ref t), // T
        ] if p.as_char() == '&' => {
            arg_name = n.clone();

            // Match the type that came out of the simple case
            match t.to_string().as_str() {
                // A &str
                "str" => {
                    tokens.append_all([
                        &simple_ref[0],
                        &simple_ref[1],
                        // replace the type with an owned string
                        &TokenTree::Ident(Ident::new("String", t.span())),
                    ]);
                },

                // Unexpected non-standard type as reference
                // (likely a known custom type generated by wit-bindgen)
                _ => {
                    // Add a modified group of tokens to the list for the struct
                    tokens.append_all([
                        &simple_ref[0], // name
                        &simple_ref[1], // colon
                        // We expect that all types that were defined by this module
                        // will be lifted to the top level, and accessible *without* fully qualified paths
                        &simple_ref[3]
                    ]);
                }
            }
        },


        // pattern: 'name: &[T]'
        arr_ref @ [
            TokenTree::Ident(ref n), // name
            TokenTree::Punct(_), // :
            TokenTree::Punct(ref p), // &
            TokenTree::Group(ref t), // [T]
        ] if p.as_char() == '&' && t.delimiter() == Delimiter::Bracket => {
            arg_name = n.clone();

            tokens.append_all([
                &arr_ref[0], // name
                &arr_ref[1], // colon
                &TokenTree::Ident(Ident::new("Vec", Span::call_site())), // Vec
                &TokenTree::Punct(Punct::new('<', Spacing::Joint)), // <
            ]);
            tokens.extend(t.stream());
            tokens.append(TokenTree::Punct(Punct::new('>', Spacing::Joint))); // >
        },

        // pattern: 'name: Wrapper<&T>'
        wrapped_ref @ [
            TokenTree::Ident(ref n),  // name
            TokenTree::Punct(_),  // :
            TokenTree::Ident(_),  // Wrapper
            TokenTree::Punct(ref p),  // <
            TokenTree::Punct(ref p2), // &
            ..,  // T
            TokenTree::Punct(_) // >
        ] if p.as_char() == '<' && p2.as_char() == '&' => {
            arg_name = n.clone();

            // Slice out the parts in between the < ... >
            let type_section = &wrapped_ref[4..wrapped_ref.len()];

            match type_section {
                // case: str (i.e. Vec<&str>)
                [
                    TokenTree::Punct(_), // <
                    TokenTree::Ident(ref n),
                    TokenTree::Punct(_) // >
                ] if n.to_string().as_str() == "str" => {
                    tokens.append_all([
                        &wrapped_ref[0], // name
                        &wrapped_ref[1], // colon
                        &wrapped_ref[2], // wrapper
                        &wrapped_ref[3], // <
                        &TokenTree::Ident(Ident::new("String", n.span())),
                        &wrapped_ref[5], // >
                    ]);
                },

                // case: [u8] (i.e. Vec<&[u8]>)
                [
                    TokenTree::Punct(_), // <
                    TokenTree::Group(g),
                    TokenTree::Punct(_), // >
                ] if g.to_string().as_str() == "[u8]" => {
                    tokens.append_all([
                        &wrapped_ref[0], // name
                        &wrapped_ref[1], // colon
                        &wrapped_ref[2], // wrapper
                        &wrapped_ref[3], // <
                        &TokenTree::Ident(Ident::new("Vec", Span::call_site())), // Vec
                        &TokenTree::Punct(Punct::new('<', Spacing::Joint)), // <
                        &TokenTree::Ident(Ident::new("u8", Span::call_site())), // u8
                        &TokenTree::Punct(Punct::new('>', Spacing::Joint)), // >
                        &TokenTree::Punct(Punct::new('>', Spacing::Joint)), // >
                    ]);
                },

                // case: T (i.e. Vec<T>)
                rest =>  {
                    let arg_type = &rest[1].to_string();

                    // If we have a < T >, and T is a struct this module defined, we must use the full path to it
                    // if not, it is likely a builtin, so we can use it directly
                    if let Some((struct_path, _)) = struct_lookup.get(arg_type) {
                        tokens.append_all(&wrapped_ref[0..5]);
                        tokens.append_all([ struct_path.to_token_stream() ]);
                        tokens.append_all(&wrapped_ref[6..]);
                    } else if let Some((type_path, _)) = type_lookup.get(arg_type) {
                        tokens.append_all(&wrapped_ref[0..5]);
                        tokens.append_all([ type_path.to_token_stream() ]);
                        tokens.append_all(&wrapped_ref[6..]);
                    } else {
                        tokens.append_all(wrapped_ref);
                    };
                },
            }
        },

        // case: Vec<(String, T)> (WIT-ified map)
        // NOTE: this only works for arguments that end in '_map'
        ts if replace_witified_maps
            && ts.len() > 2 // in order to skip the name & colon tokens
            && matches!(ts[0], TokenTree::Ident(ref n) if n.to_string().ends_with("_map"))
            && extract_witified_map(&ts[2..]).is_some() => {
                let raw_arg_name = ts[0].to_string();
                arg_name = Ident::new(raw_arg_name.as_str(), ts[0].span());
                // For maps that are replaced by bindgen config, we want to replace the `_map` suffix
                //
                // i.e. the "some_map" in `some_map: list<(string,string)>` becomes "some" for Rust code
                let trimmed_arg_name = Ident::new(raw_arg_name.trim_end_matches("_map"), ts[0].span());
                let map_type = extract_witified_map(&ts[2..]).expect("failed to parse WIT-ified map type");
                tokens.append_all(quote::quote!(#trimmed_arg_name: #map_type));
            },

        // pattern: unknown (any T)
        ts => {
            // Save the first token (which should be the argument name) as an invocation argument for later
            if let TokenTree::Ident(name) = &ts[0] {
                arg_name = name.clone();
            } else {
                panic!("unexpectedly missing the first token in FnArg");
            }

            // With a completely unknown type, we should attempt to replace it with a qualified type name
            match &ts[2] {
                // If the third token (after the arg name and ':') has a type we know, fill in the qualified name
                TokenTree::Ident(name) if type_lookup.contains_key(&name.to_string()) => {
                    let qualified_type = type_lookup.get(&name.to_string()).unwrap().0.to_token_stream();
                    tokens.append_all(&ts[0..2]);
                    tokens.append_all(qualified_type);
                    tokens.append_all(&ts[3..]);
                }
                // Ignore types that aren't in some lookup
                _ => tokens.append_all(ts)
            }
        }
    }

    (arg_name, tokens)
}
