use anyhow::{Context as _, Result};
use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{AttrStyle, Attribute, Expr, ExprLit, ExprPath, Lit, Path};
use tracing::{debug, error};

pub(crate) const WRPC_DERIVE_CONFIG_ATTR_NAME: &str = "wrpc_transport_derive";

/// Options that can be specified via attribute (i.e [`syn::Attribute`]) when performing derivation
pub(crate) struct AttrOptions {
    /// Path to the crate that will be used in macro expansion (ex. `::some::other::path`)
    /// By default, this is `::wrpc_derive_macro`
    pub crate_path: TokenStream,
}

impl AttrOptions {
    /// Parse options specified by attribute for deriving
    pub(crate) fn try_from_attributes<I>(attrs: I) -> Result<AttrOptions>
    where
        for<'a> &'a I: IntoIterator<Item = &'a Attribute>,
    {
        let mut crate_path = quote!(::wrpc_transport_derive);

        // Parse out top level attributes that influence behavior
        for attr in attrs.into_iter() {
            if attr.style == AttrStyle::Outer && attr.path().is_ident(WRPC_DERIVE_CONFIG_ATTR_NAME)
            {
                match attr.parse_args::<syn::Expr>() {
                    Ok(expr) => {
                        if let Expr::Assign(ea) = expr {
                            match (*ea.left, *ea.right) {
                                // #[wrpc_transport_derive(crate = "...")]
                                (
                                    Expr::Path(ExprPath { path, .. }),
                                    Expr::Lit(ExprLit {
                                        lit: Lit::Str(s), ..
                                    }),
                                ) if path.is_ident("crate") => {
                                    debug!("found custom crate path: [{}]", s.value());
                                    crate_path = s
                                        .parse::<Path>()
                                        .with_context(|| {
                                            format!(
                                                "failed to parse custom crate path [{}]",
                                                s.value()
                                            )
                                        })?
                                        .to_token_stream();
                                }
                                // Ignore other unrecognized attributes
                                _ => {}
                            }
                        }
                    }
                    // Ignore other types of expressions used in attributes
                    Err(e) => {
                        error!("unexpectedly failed to parse attr: {e}");
                    }
                }
            }
        }

        Ok(AttrOptions { crate_path })
    }
}
