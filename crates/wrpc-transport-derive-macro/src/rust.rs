use anyhow::{anyhow, Context as _, Result};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;

/// Check if a given type has a wrapped type (ex. Option<T>, Vec<T>)
pub(crate) fn has_wrapped_type(
    ty: syn::Type,
    expected: impl AsRef<str>,
) -> Result<Option<syn::Type>> {
    let mut tt = ty.to_token_stream().into_iter().collect::<Vec<TokenTree>>();
    match &mut tt[..] {
        // If we can see the Wrapper<T> pattern, we can extract the inner type
        [
            TokenTree::Ident(w),  // Wrapper (Option)
            TokenTree::Punct(ref p),  // <
            ..,  // T
            TokenTree::Punct(_) // >
        ] if *w == expected.as_ref() && p.as_char() == '<' => {
            let inner_ty = syn::parse2::<syn::Type>(TokenStream::from_iter(tt.drain(2..tt.len()-1).collect::<Vec<TokenTree>>()))
                .map_err(|e| anyhow!(e))
                .context("failed to parse type out of wrapper")?;
            Ok(Some(inner_ty))
        },
        // If we didn't match, then there's no inner type/this isn't a wrapper
        _ => Ok(None),
    }
}

/// Check if a given [`syn::Type`] is an optional (i.e. Option<T>), returning the inner type
pub(crate) fn has_option_type(ty: syn::Type) -> Result<Option<syn::Type>> {
    has_wrapped_type(ty, "Option")
}

/// Check if a given [`syn::Type`] is an list type (i.e. Vec<T>), returning the inner type
pub(crate) fn has_vec_type(ty: syn::Type) -> Result<Option<syn::Type>> {
    has_wrapped_type(ty, "Vec")
}

#[cfg(test)]
mod tests {
    use anyhow::{anyhow, Context as _, Result};
    use quote::quote;
    use syn::Type;

    use super::{has_option_type, has_vec_type};

    /// Detecting a Option should work for a appropriately wrapped type (ex. Option<String>)
    #[test]
    fn option_type_detection() -> Result<()> {
        let ty: Type = syn::parse2(quote!(Option<String>)).map_err(|e| anyhow!(e))?;
        let expected_ty: Type = syn::parse2(quote!(String)).map_err(|e| anyhow!(e))?;
        assert_eq!(
            has_option_type(ty)
                .context("failed to check")?
                .context("missing type")?,
            expected_ty
        );
        Ok(())
    }

    /// Detecting a Vec should fail for a non-wrapped type (ex. String)
    #[test]
    fn option_type_detection_fail() -> Result<()> {
        let ty: Type = syn::parse2(quote!(String)).map_err(|e| anyhow!(e))?;
        assert!(matches!(has_option_type(ty), Ok(None)));
        Ok(())
    }

    /// Detecting a Option should fail for a different wrapper type (ex. Vec<T>)
    #[test]
    fn option_type_detection_fail_vec() -> Result<()> {
        let ty: Type = syn::parse2(quote!(Vec<String>)).map_err(|e| anyhow!(e))?;
        assert!(matches!(has_option_type(ty), Ok(None)));
        Ok(())
    }

    /// Detecting a Vec should work for a appropriately wrapped type (ex. Vec<String>)
    #[test]
    fn vec_type_detection() -> Result<()> {
        let ty: Type = syn::parse2(quote!(Vec<String>)).map_err(|e| anyhow!(e))?;
        let expected_ty: Type = syn::parse2(quote!(String)).map_err(|e| anyhow!(e))?;
        assert_eq!(
            has_vec_type(ty)
                .context("failed to check")?
                .context("missing type")?,
            expected_ty
        );
        Ok(())
    }

    /// Detecting a Vec should fail for a non-wrapped type (ex. String)
    #[test]
    fn vec_type_detection_fail() -> Result<()> {
        let ty: Type = syn::parse2(quote!(String)).map_err(|e| anyhow!(e))?;
        assert!(matches!(has_vec_type(ty), Ok(None)));
        Ok(())
    }

    /// Detecting a Vec should fail for a different wrapper type (ex. Option<T>)
    #[test]
    fn vec_type_detection_fail_option() -> Result<()> {
        let ty: Type = syn::parse2(quote!(Option<String>)).map_err(|e| anyhow!(e))?;
        assert!(matches!(has_vec_type(ty), Ok(None)));
        Ok(())
    }
}
