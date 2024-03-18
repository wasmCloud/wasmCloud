use anyhow::{anyhow, bail, Context as _, Result};
use proc_macro2::{Span, TokenStream};
use quote::format_ident;
use syn::{Fields, FieldsNamed, FieldsUnnamed, Ident, LitInt};
use tracing::warn;

use crate::config::AttrOptions;

/// Derive [`wrpc_transport::Encode`] for a given input stream
pub fn derive_encode_inner(input: TokenStream) -> Result<TokenStream> {
    let item = syn::parse2::<syn::Item>(input)
        .map_err(|e| anyhow!(e))
        .context("failed to parse input into item")?;

    // Depending on the type of struct, generate the impl
    match item {
        // For enums we generate an impl of Encode
        syn::Item::Enum(_) => {
            derive_encode_inner_for_enum(item).context("failed to derive for enum")
        }

        // For structs we generate an impl for Encode
        syn::Item::Struct(_) => {
            derive_encode_inner_for_struct(item).context("failed to derive for struct")
        }

        // All other types of syntax tree are not allowed
        _ => {
            warn!("derive(Encode) does not support this syntax item");
            Ok(TokenStream::new())
        }
    }
}

/// Derive `Encode` for a Rust struct
fn derive_encode_inner_for_struct(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Struct(s) = item else {
        bail!("provided syn::Item is not a struct");
    };

    let struct_name = s.ident;
    let mut members = Vec::<Ident>::new();
    let mut encode_lines = Vec::<TokenStream>::new();

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(s.attrs)?;

    // For each member of the struct, we must:
    // - generate a line that attempts to encode the member
    // - remember the type so we can require it to be Encode
    for member in s.fields.iter() {
        let member_name = member
            .ident
            .clone()
            .context("unexpectedly missing field name in struct")?;
        members.push(member_name.clone());
        encode_lines.push(quote::quote!(
            #member_name.encode(&mut payload).await.context("failed to encode member `#member_name`")?;
        ));
    }

    // Build the generated impl
    Ok(quote::quote!(
        #[automatically_derived]
        #[#crate_path::deps::async_trait::async_trait]
        impl #crate_path::deps::wrpc_transport::Encode for #struct_name
        {
            async fn encode(
                self,
                mut payload: &mut (impl #crate_path::deps::bytes::buf::BufMut + Send)
            ) -> #crate_path::deps::anyhow::Result<Option<#crate_path::deps::wrpc_transport::AsyncValue>> {
                use #crate_path::deps::anyhow::Context as _;
                let Self { #( #members ),* } = self;
                #( #encode_lines );*
                Ok(None)
            }
        }
    ))
}

/// Derive `Encode` for a Rust struct
fn derive_encode_inner_for_enum(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Enum(e) = item else {
        bail!("provided syn::Item is not an enum");
    };

    let enum_name = e.ident;
    let mut variant_encode_lines = Vec::<TokenStream>::new();

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(e.attrs)?;

    // For each variant, we must do two things:
    // - ensure that the type we're about to use is Encode
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
                    // Every type that is used inside must be constrained to ensure Encode 
                    // For every named field we must do two things:
                    // - keep name of the arg (for listing later)
                    // - build a line that does the encode sync
                    let named_field_name = named_field
                        .ident
                        .clone()
                        .context("unexpectedly missing named field")?;
                    args.push(named_field_name.clone());
                    named_field_encode_lines.push(quote::quote!(
                        #named_field_name.encode(&mut payload).await?;
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
                    // Every type that is used inside must be constrained to ensure Encode 
                    // For every unnamed field we must do two things:
                    // - keep name of the arg (for listing later)
                    // - build a line that does the encode sync
                    let unnamed_field_arg_name = format_ident!("arg{}", unnamed_field_idx);
                    args.push(unnamed_field_arg_name.clone());
                    unnamed_field_encode_lines.push(quote::quote!(
                        #unnamed_field_arg_name.encode(&mut payload).await?;
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
                Self::#name => {
                    #crate_path::deps::wrpc_transport::encode_discriminant(&mut payload, #idx_ident)?;
                }
            )),
        }
    }

    Ok(quote::quote!(
        #[automatically_derived]
        #[#crate_path::deps::async_trait::async_trait]
        impl #crate_path::deps::wrpc_transport::Encode for #enum_name
        {
            async fn encode(
                self,
                mut payload: &mut (impl #crate_path::deps::bytes::buf::BufMut + Send)
            ) -> #crate_path::deps::anyhow::Result<Option<#crate_path::deps::wrpc_transport::AsyncValue>> {
                match self {
                    #(
                        #variant_encode_lines
                    ),*
                }
                Ok(None)
            }
        }
    ))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::wrpc_transport::encode::derive_encode_inner;

    #[test]
    fn encode_struct_simple() -> Result<()> {
        let derived = derive_encode_inner(quote::quote!(
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
        let derived = derive_encode_inner(quote::quote!(
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
        let derived = derive_encode_inner(quote::quote!(
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
        let derived = derive_encode_inner(quote::quote!(
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
        let derived = derive_encode_inner(quote::quote!(
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
        let derived = derive_encode_inner(quote::quote!(
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
        let derived = derive_encode_inner(quote::quote!(
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
