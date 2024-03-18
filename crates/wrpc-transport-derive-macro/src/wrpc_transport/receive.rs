use anyhow::{anyhow, bail, Context as _, Result};
use proc_macro2::{Span, TokenStream};
use quote::format_ident;
use syn::{Fields, FieldsNamed, FieldsUnnamed, Ident, LitInt, LitStr};
use tracing::warn;

use crate::config::AttrOptions;

/// Derive [`wrpc_transport::Receive`] for a given input stream
pub fn derive_receive_inner(input: TokenStream) -> Result<TokenStream> {
    let item = syn::parse2::<syn::Item>(input)
        .map_err(|e| anyhow!(e))
        .context("failed to parse input into item")?;

    // Depending on the type of struct, generate the impl
    match item {
        // For enums we generate an impl of Receive
        syn::Item::Enum(_) => {
            derive_receive_inner_for_enum(item).context("failed to derive for enum")
        }

        // For structs we generate an impl for Receive
        syn::Item::Struct(_) => {
            derive_receive_inner_for_struct(item).context("failed to derive for struct")
        }

        // All other types of syntax tree are not allowed
        _ => {
            warn!("derive(Receive) does not support this syntax item");
            Ok(TokenStream::new())
        }
    }
}

/// Derive [`wrpc_transport::Subscribe`] for a given input stream
pub fn derive_subscribe_inner(input: TokenStream) -> Result<TokenStream> {
    let item = syn::parse2::<syn::Item>(input)
        .map_err(|e| anyhow!(e))
        .context("failed to parse input into item")?;

    // Depending on the type of struct, generate the impl
    match item {
        // For enums we generate an impl of Subscribe
        syn::Item::Enum(_) => {
            derive_subscribe_inner_for_enum(item).context("failed to derive for enum")
        }

        // For structs we generate an impl for Subscribe
        syn::Item::Struct(_) => {
            derive_subscribe_inner_for_struct(item).context("failed to derive for struct")
        }

        // All other types of syntax tree are not allowed
        _ => {
            warn!("derive(Subscribe) does not support this syntax item");
            Ok(TokenStream::new())
        }
    }
}

/// Derive `Subscribe` for a Rust struct
fn derive_subscribe_inner_for_struct(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Struct(s) = item else {
        bail!("provided syn::Item is not a struct");
    };

    let struct_name = s.ident;

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(s.attrs)?;

    // Build the generated impl
    Ok(quote::quote!(
        #[automatically_derived]
        impl #crate_path::deps::wrpc_transport::Subscribe for #struct_name
        {
            async fn subscribe<T: #crate_path::deps::wrpc_transport::Subscriber + Send + Sync>(
                subscriber: &T,
                subject: T::Subject,
            ) -> Result<Option<#crate_path::deps::wrpc_transport::AsyncSubscription<T::Stream>>, T::SubscribeError> {
                Ok(None)
            }
        }
    ))
}

/// Derive `Subscribe` for a Rust struct
fn derive_subscribe_inner_for_enum(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Enum(e) = item else {
        bail!("provided syn::Item is not an enum");
    };

    let enum_name = e.ident;

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(e.attrs)?;

    Ok(quote::quote!(
        #[automatically_derived]
        impl #crate_path::deps::wrpc_transport::Subscribe for #enum_name
        {
            async fn subscribe<T: #crate_path::deps::wrpc_transport::Subscriber + Send + Sync>(
                subscriber: &T,
                subject: T::Subject,
            ) -> Result<Option<#crate_path::deps::wrpc_transport::AsyncSubscription<T::Stream>>, T::SubscribeError> {
                Ok(None)
            }
        }
    ))
}

/// Derive `Receive` for a Rust struct
fn derive_receive_inner_for_struct(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Struct(s) = item else {
        bail!("provided syn::Item is not a struct");
    };

    let struct_name = s.ident;
    let mut members = Vec::<Ident>::new();
    let mut receive_lines = Vec::<TokenStream>::new();

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(s.attrs)?;

    // For each member of the struct, we must:
    // - generate a line that attempts to receive the member
    // - remember the type so we can require it to be Receive
    for member in s.fields.iter() {
        let member_name = member
            .ident
            .clone()
            .context("unexpectedly missing field name in struct")?;
        let member_name_lit_str = LitStr::new(member_name.to_string().as_ref(), Span::call_site());
        members.push(member_name.clone());
        // Add a line that receives this member
        receive_lines.push(quote::quote!(
            let (#member_name, payload) = #crate_path::deps::wrpc_transport::Receive::receive_sync(payload, rx)
                .await
                .with_context(|| format!("failed to receive member `{}`", #member_name_lit_str))?;
        ));
    }

    // Build the generated impl
    Ok(quote::quote!(
        #[automatically_derived]
        #[#crate_path::deps::async_trait::async_trait]
        impl<'a> #crate_path::deps::wrpc_transport::Receive<'a> for #struct_name
        {
            async fn receive<T>(
                payload: impl #crate_path::deps::bytes::buf::Buf + Send + 'a,
                rx: &mut (impl #crate_path::deps::futures::Stream<Item=#crate_path::deps::anyhow::Result<#crate_path::deps::bytes::Bytes>>  + Send + Sync + Unpin),
                _sub: Option<#crate_path::deps::wrpc_transport::AsyncSubscription<T>>,
            ) -> #crate_path::deps::anyhow::Result<(Self, Box<dyn #crate_path::deps::bytes::buf::Buf + Send + 'a>)>
            where
                T: #crate_path::deps::futures::Stream<Item=#crate_path::deps::anyhow::Result<#crate_path::deps::bytes::Bytes>> + Send + Sync + 'static
            {
                use #crate_path::deps::anyhow::Context as _;
                #( #receive_lines );*
                Ok((Self { #( #members ),* }, Box::new(payload) ))
            }
        }
    ))
}

/// Derive `Receive` for a Rust struct
fn derive_receive_inner_for_enum(item: syn::Item) -> Result<TokenStream> {
    let syn::Item::Enum(e) = item else {
        bail!("provided syn::Item is not an enum");
    };

    let enum_name = e.ident;
    let enum_name_str = LitStr::new(enum_name.to_string().as_ref(), Span::call_site());
    let mut variant_receive_match_blocks = Vec::<TokenStream>::new();

    let AttrOptions { crate_path } = AttrOptions::try_from_attributes(e.attrs)?;

    // For each variant, we must do two things:
    // - ensure that the type we're about to use is Receive
    // - generate a line that receives the variant
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
                let mut named_field_receive_lines = Vec::<TokenStream>::new();

                // For each named field, generate a `receive_sync()` call that will fit
                // in the block of an option in the large match statement for this enum
                for named_field in named.iter() {
                    // For every named field we must do two things:
                    // - keep name of the arg (for listing later)
                    // - build a line that does the receive sync
                    let named_field_name = named_field
                        .ident
                        .clone()
                        .context("unexpectedly missing named field")?;
                    args.push(named_field_name.clone());
                    named_field_receive_lines.push(quote::quote!(
                        let (#named_field_name, payload) = #crate_path::deps::wrpc_transport::Receive::receive_sync(payload, rx)
                            .await
                            .context("failed to receive enum discriminant inner value")?;
                    ));
                }

                // Generate match statement block
                variant_receive_match_blocks.push(quote::quote!(
                    #idx_ident => {
                        #( #named_field_receive_lines );*
                        Ok((Self::#name { #( #args ),* }, Box::new(payload)))
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
                let mut unnamed_field_receive_lines = Vec::<TokenStream>::new();

                // For each unnamed field, generate a `receive_sync()` call that will fit
                // in the block of an option in the large match statement for this enum
                for (unnamed_field_idx, _) in unnamed.iter().enumerate() {
                    // For every unnamed field we must do two things:
                    // - keep name of the arg (for listing later)
                    // - build a line that does the receive sync
                    let unnamed_field_arg_name = format_ident!("arg{}", unnamed_field_idx);
                    args.push(unnamed_field_arg_name.clone());
                    unnamed_field_receive_lines.push(quote::quote!(
                        let (#unnamed_field_arg_name, payload) = #crate_path::deps::wrpc_transport::Receive::receive_sync(payload, rx)
                            .await
                            .context("failed to receive enum discriminant inner value")?;
                    ));
                }

                // Generate match statement block
                variant_receive_match_blocks.push(quote::quote!(
                    #idx_ident => {
                        #( #unnamed_field_receive_lines );*
                        Ok((Self::#name(#( #args ),*), Box::new(payload)))
                    }
                ))
            }
            // If there are no fields we can just receive the discriminant
            Fields::Unit => variant_receive_match_blocks.push(quote::quote!(
                #idx_ident => Ok((Self::#name, Box::new(payload)))
            )),
        }
    }

    Ok(quote::quote!(
        #[#crate_path::deps::async_trait::async_trait]
        impl<'a> #crate_path::deps::wrpc_transport::Receive<'a> for #enum_name
        {
            async fn receive<T>(
                payload: impl #crate_path::deps::bytes::buf::Buf + Send + 'a,
                rx: &mut (impl #crate_path::deps::futures::Stream<Item=#crate_path::deps::anyhow::Result<#crate_path::deps::bytes::Bytes>> + Send + Sync + Unpin),
                _sub: Option<#crate_path::deps::wrpc_transport::AsyncSubscription<T>>,
            ) -> #crate_path::deps::anyhow::Result<(Self, Box<dyn #crate_path::deps::bytes::buf::Buf + Send + 'a>)>
            where
                T: #crate_path::deps::futures::Stream<Item=#crate_path::deps::anyhow::Result<#crate_path::deps::bytes::Bytes>> + Send + Sync + 'static
            {
                use #crate_path::deps::anyhow::Context as _;
                let (discriminant, payload) = #crate_path::deps::wrpc_transport::receive_discriminant(payload, rx)
                    .await
                    .context("failed to receive enum discriminant")?;
                match discriminant {
                    #(
                        #variant_receive_match_blocks
                    ),*,
                    v => #crate_path::deps::anyhow::bail!(
                        "unexpected discriminant value on type [{}]: [{v}]", #enum_name_str
                    ),
                }
            }
        }
    ))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::wrpc_transport::receive::derive_receive_inner;

    #[test]
    fn derive_receive_inner_works() -> Result<()> {
        let tokens = quote::quote!(
            struct Test {
                byte: u8,
            }
        );
        let parsed_item = syn::parse2::<syn::Item>(derive_receive_inner(tokens)?);
        assert!(matches!(parsed_item, Ok(syn::Item::Impl(_))));
        Ok(())
    }
}
