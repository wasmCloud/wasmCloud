use anyhow::{anyhow, bail, Context as _, Result};
use proc_macro2::{Span, TokenStream};
use quote::format_ident;
use syn::{Fields, FieldsNamed, FieldsUnnamed, Ident, LitInt};
use tracing::warn;

use crate::config::AttrOptions;
use crate::rust::{has_option_type, has_vec_type};

/// Derive [`wrpc_transport::EncodeSync`] for a given input stream
pub fn derive_encode_sync_inner(input: TokenStream) -> Result<TokenStream> {
    let item = syn::parse2::<syn::Item>(input)
        .map_err(|e| anyhow!(e))
        .context("failed to parse input into item")?;

    // Depending on the type of struct, generate the impl
    match item {
        // For enums we generate an impl of EncodeSync
        syn::Item::Enum(_) => {
            derive_encode_sync_inner_for_enum(item).context("failed to derive for enum")
        }

        // For structs we generate an impl for EncodeSync
        syn::Item::Struct(_) => {
            derive_encode_sync_inner_for_struct(item).context("failed to derive for struct")
        }

        // All other types of syntax tree are not allowed
        _ => {
            warn!("derive(EncodeSync) does not support this syntax item");
            Ok(TokenStream::new())
        }
    }
}

/// Derive `EncodeSync` for a Rust struct
fn derive_encode_sync_inner_for_struct(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Struct(s) = item else {
        bail!("provided syn::Item is not a struct");
    };

    let struct_name = s.ident;
    let mut members = Vec::<Ident>::new();
    let mut encode_lines = Vec::<TokenStream>::new();

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(s.attrs)?;

    // For each member of the struct, we must:
    // - generate a line that attempts to encode the member
    // - remember the type so we can require it to be EncodeSync
    for member in s.fields.iter() {
        let member_name = member
            .ident
            .clone()
            .context("unexpectedly missing field name in struct")?;
        members.push(member_name.clone());
        if let Ok(Some(_)) = has_option_type(member.ty.clone()) {
            encode_lines.push(quote::quote!(
                #crate_path::deps::wrpc_transport::EncodeSync::encode_sync_option(#member_name, &mut payload).context("failed to encode member (option) `#member_name`")?;
            ));
        } else if let Ok(Some(_)) = has_vec_type(member.ty.clone()) {
            encode_lines.push(quote::quote!(
                #crate_path::deps::wrpc_transport::EncodeSync::encode_sync_list(#member_name, &mut payload).context("failed to encode member (list) `#member_name`")?;
            ));
        } else {
            encode_lines.push(quote::quote!(
                #member_name.encode_sync(&mut payload).context("failed to encode member `#member_name`")?;
            ));
        }
    }

    // Build the generated impl
    Ok(quote::quote!(
        #[automatically_derived]
        impl #crate_path::deps::wrpc_transport::EncodeSync for #struct_name
        {
            fn encode_sync(
                self,
                mut payload: impl #crate_path::deps::bytes::buf::BufMut
            ) -> #crate_path::deps::anyhow::Result<()> {
                use #crate_path::deps::anyhow::Context as _;
                let Self { #( #members ),* } = self;
                #( #encode_lines );*
                Ok(())
            }
        }
    ))
}

/// Derive `EncodeSync` for a Rust struct
fn derive_encode_sync_inner_for_enum(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Enum(e) = item else {
        bail!("provided syn::Item is not an enum");
    };

    let enum_name = e.ident;
    let mut variant_encode_lines = Vec::<TokenStream>::new();

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(e.attrs)?;

    // For each variant, we must do two things:
    // - ensure that the type we're about to use is EncodeSync
    // - generate a line that encodes the variant
    for (idx, v) in e.variants.iter().enumerate() {
        let name = &v.ident;
        let idx_ident = LitInt::new(&idx.to_string(), Span::call_site());
        match &v.fields {
            // For named fields, we have a variant that looks like:
            //
            // ```
            // enum Example {
            //     ...
            //     Variant { some: u32, fields: u64 }
            //     ...
            // }
            // ```
            //
            // We need to go through and generate lines for every inner field there
            Fields::Named(FieldsNamed { named, .. }) => {
                let mut args = Vec::<Ident>::new();
                let mut named_field_encode_lines = Vec::<TokenStream>::new();
                for named_field in named.iter() {
                    // Every type that is used inside must be constrained to ensure EncodeSync
                    // For every named field we must do two things:
                    // - keep name of the arg (for listing later)
                    // - build a line that does the encode sync
                    let named_field_name = named_field
                        .ident
                        .clone()
                        .context("unexpectedly missing named field")?;
                    args.push(named_field_name.clone());
                    named_field_encode_lines.push(quote::quote!(
                        #named_field_name.encode_sync(&mut payload)?
                    ));
                }

                variant_encode_lines.push(quote::quote!(
                    Self::#name { #( #args ),* } => {
                        #crate_path::deps::wrpc_transport::encode_discriminant(&mut payload, #idx_ident)?;
                        #( #named_field_encode_lines );*
                    }
                ))
            }

            // For variants with unnamed fields, we have a variant that looks like:
            //
            // ```
            // enum Example {
            //     ...
            //     Variant(u32, u64)
            //     ...
            // }
            // ```
            //
            // We need to go through and generate lines for every inner field there,
            // giving them a name as we go
            Fields::Unnamed(FieldsUnnamed { unnamed, .. }) => {
                let mut args = Vec::<Ident>::new();
                let mut unnamed_field_encode_lines = Vec::<TokenStream>::new();
                for (unnamed_field_idx, _) in unnamed.iter().enumerate() {
                    // Every type that is used inside must be constrained to ensure EncodeSync
                    // For every unnamed field we must do two things:
                    // - keep name of the arg (for listing later)
                    // - build a line that does the encode sync
                    let unnamed_field_arg_name = format_ident!("arg{}", unnamed_field_idx);
                    args.push(unnamed_field_arg_name.clone());
                    unnamed_field_encode_lines.push(quote::quote!(
                        #unnamed_field_arg_name.encode_sync(&mut payload)?
                    ));
                }

                variant_encode_lines.push(quote::quote!(
                    Self::#name( #( #args ),* ) => {
                        #crate_path::deps::wrpc_transport::encode_discriminant(&mut payload, #idx_ident)?;
                        #( #unnamed_field_encode_lines );*
                    }
                ))
            }
            // If there are no fields we can just encode the discriminant
            Fields::Unit => variant_encode_lines.push(quote::quote!(
                Self::#name => #crate_path::deps::wrpc_transport::encode_discriminant(&mut payload, #idx_ident)?
            )),
        }
    }

    Ok(quote::quote!(
        impl #crate_path::deps::wrpc_transport::EncodeSync for #enum_name
        {
            fn encode_sync(
                self,
                mut payload: impl #crate_path::deps::bytes::buf::BufMut
            ) -> #crate_path::deps::anyhow::Result<()> {
                match self {
                    #(
                        #variant_encode_lines
                    ),*
                }
                Ok(())
            }
        }
    ))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::wrpc_transport::encode_sync::derive_encode_sync_inner;

    #[test]
    fn encode_struct_simple() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            struct Test {
                byte: u8,
                string: String,
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }

    #[test]
    fn encode_struct_with_option() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            struct Test {
                byte: u8,
                string: String,
                maybe_string: Option<String>,
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }

    #[test]
    fn encode_struct_with_vec() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            struct Test {
                byte: u8,
                string: String,
                strings: Vec<String>,
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }

    #[test]
    fn encode_enum_simple() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            enum Simple {
                A,
                B,
                C,
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }

    #[test]
    fn encode_enum_unnamed_variant_args() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            enum UnnamedVariants {
                A,
                B(String, String),
                C,
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }

    #[test]
    fn encode_enum_named_variant_args() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            enum NamedVariants {
                A,
                B { first: String, second: String },
                C,
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }

    #[test]
    fn encode_enum_mixed_variant_args() -> Result<()> {
        let derived = derive_encode_sync_inner(quote::quote!(
            enum MixedVariants {
                A,
                B { first: String, second: String },
                C(String, String),
            }
        ))?;
        let parsed_item = syn::parse2::<syn::Item>(derived);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }
}
