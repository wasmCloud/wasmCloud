//! A copied version of [wasmtime-component-macro](https://github.com/bytecodealliance/wasmtime/blob/main/crates/component-macro/src/bindgen.rs),
//! as of 2023/11/23, version 15.0.0
//!
//! This version exists because we must use the bindgen functionality to parse WIT files that are used
//! by the wasmCloud bindgen macro.
//!
//! Resolving the input that would normally be processed by
//! wasmtime::component::bindgen(...) into a Config structure, and expanding it, so we can
//! manipulate the outputted code.
//!
//! We achieve the above primarily by marking data, members and functions pub(crate)
//!

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use syn::parse::{Error, Parse, ParseStream, Result};
use syn::punctuated::Punctuated;
use syn::{braced, token, Token};
use wasmtime_wit_bindgen::{AsyncConfig, Opts, Ownership, TrappableError};
use wit_parser::{PackageId, Resolve, UnresolvedPackage, WorldId};

pub struct Config {
    opts: Opts,
    pub(crate) resolve: Resolve,
    world: WorldId,
    files: Vec<PathBuf>,
}

pub fn expand(input: &Config) -> Result<TokenStream> {
    if !cfg!(feature = "async") && input.opts.async_.maybe_async() {
        return Err(Error::new(
            Span::call_site(),
            "cannot enable async bindings unless `async` crate feature is active",
        ));
    }

    let src = input.opts.generate(&input.resolve, input.world);
    let mut contents = src.parse::<TokenStream>().unwrap();

    // Include a dummy `include_str!` for any files we read so rustc knows that
    // we depend on the contents of those files.
    for file in input.files.iter() {
        contents.extend(
            format!("const _: &str = include_str!(r#\"{}\"#);\n", file.display())
                .parse::<TokenStream>()
                .unwrap(),
        );
    }

    Ok(contents)
}

impl Parse for Config {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let call_site = Span::call_site();
        let mut opts = Opts::default();
        let mut world = None;
        let mut inline = None;
        let mut path = None;
        let mut async_configured = false;

        if input.peek(token::Brace) {
            let content;
            syn::braced!(content in input);
            let fields = Punctuated::<Opt, Token![,]>::parse_terminated(&content)?;
            for field in fields.into_pairs() {
                match field.into_value() {
                    Opt::Path(s) => {
                        if path.is_some() {
                            return Err(Error::new(s.span(), "cannot specify second path"));
                        }
                        path = Some(s.value());
                    }
                    Opt::World(s) => {
                        if world.is_some() {
                            return Err(Error::new(s.span(), "cannot specify second world"));
                        }
                        world = Some(s.value());
                    }
                    Opt::Inline(s) => {
                        if inline.is_some() {
                            return Err(Error::new(s.span(), "cannot specify second source"));
                        }
                        inline = Some(s.value());
                    }
                    Opt::Tracing(val) => opts.tracing = val,
                    Opt::Async(val, span) => {
                        if async_configured {
                            return Err(Error::new(span, "cannot specify second async config"));
                        }
                        async_configured = true;
                        opts.async_ = val;
                    }
                    Opt::TrappableErrorType(val) => opts.trappable_error_type = val,
                    Opt::Ownership(val) => opts.ownership = val,
                    Opt::Interfaces(s) => {
                        if inline.is_some() {
                            return Err(Error::new(s.span(), "cannot specify a second source"));
                        }
                        inline = Some(format!(
                            "
                                package wasmtime:component-macro-synthesized;

                                world interfaces {{
                                    {}
                                }}
                            ",
                            s.value()
                        ));

                        if world.is_some() {
                            return Err(Error::new(
                                s.span(),
                                "cannot specify a world with `interfaces`",
                            ));
                        }
                        world = Some("interfaces".to_string());

                        opts.only_interfaces = true;
                    }
                    Opt::With(val) => opts.with.extend(val),
                }
            }
        } else {
            world = input.parse::<Option<syn::LitStr>>()?.map(|s| s.value());
            if input.parse::<Option<syn::token::In>>()?.is_some() {
                path = Some(input.parse::<syn::LitStr>()?.value());
            }
        }
        let (resolve, pkg, files) = parse_source(&path, &inline)
            .map_err(|err| Error::new(call_site, format!("{err:?}")))?;

        let world = resolve
            .select_world(pkg, world.as_deref())
            .map_err(|e| Error::new(call_site, format!("{e:?}")))?;
        Ok(Config {
            opts,
            resolve,
            world,
            files,
        })
    }
}

fn parse_source(
    path: &Option<String>,
    inline: &Option<String>,
) -> anyhow::Result<(Resolve, PackageId, Vec<PathBuf>)> {
    let mut resolve = Resolve::default();
    let mut files = Vec::new();
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    let mut parse = |resolve: &mut Resolve, path: &Path| -> anyhow::Result<_> {
        if path.is_dir() {
            let (pkg, sources) = resolve.push_dir(path)?;
            files = sources;
            Ok(pkg)
        } else {
            let pkg = UnresolvedPackage::parse_file(path)?;
            files.extend(pkg.source_files().map(|s| s.to_owned()));
            resolve.push(pkg)
        }
    };

    let path_pkg = if let Some(path) = path {
        Some(parse(&mut resolve, &root.join(path))?)
    } else {
        None
    };

    let inline_pkg = if let Some(inline) = inline {
        Some(resolve.push(UnresolvedPackage::parse("macro-input".as_ref(), inline)?)?)
    } else {
        None
    };

    let pkg = inline_pkg
        .or(path_pkg)
        .map_or_else(|| parse(&mut resolve, &root.join("wit")), Ok)?;

    Ok((resolve, pkg, files))
}

mod kw {
    syn::custom_keyword!(inline);
    syn::custom_keyword!(path);
    syn::custom_keyword!(tracing);
    syn::custom_keyword!(trappable_error_type);
    syn::custom_keyword!(world);
    syn::custom_keyword!(ownership);
    syn::custom_keyword!(interfaces);
    syn::custom_keyword!(with);
    syn::custom_keyword!(except_imports);
    syn::custom_keyword!(only_imports);
}

enum Opt {
    World(syn::LitStr),
    Path(syn::LitStr),
    Inline(syn::LitStr),
    Tracing(bool),
    Async(AsyncConfig, Span),
    TrappableErrorType(Vec<TrappableError>),
    Ownership(Ownership),
    Interfaces(syn::LitStr),
    With(HashMap<String, String>),
}

impl Parse for Opt {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let l = input.lookahead1();
        if l.peek(kw::path) {
            input.parse::<kw::path>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Path(input.parse()?))
        } else if l.peek(kw::inline) {
            input.parse::<kw::inline>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Inline(input.parse()?))
        } else if l.peek(kw::world) {
            input.parse::<kw::world>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::World(input.parse()?))
        } else if l.peek(kw::tracing) {
            input.parse::<kw::tracing>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Tracing(input.parse::<syn::LitBool>()?.value))
        } else if l.peek(Token![async]) {
            let span = input.parse::<Token![async]>()?.span;
            input.parse::<Token![:]>()?;
            if input.peek(syn::LitBool) {
                match input.parse::<syn::LitBool>()?.value {
                    true => Ok(Opt::Async(AsyncConfig::All, span)),
                    false => Ok(Opt::Async(AsyncConfig::None, span)),
                }
            } else {
                let contents;
                syn::braced!(contents in input);

                let l = contents.lookahead1();
                let ctor: fn(HashSet<String>) -> AsyncConfig = if l.peek(kw::except_imports) {
                    contents.parse::<kw::except_imports>()?;
                    contents.parse::<Token![:]>()?;
                    AsyncConfig::AllExceptImports
                } else if l.peek(kw::only_imports) {
                    contents.parse::<kw::only_imports>()?;
                    contents.parse::<Token![:]>()?;
                    AsyncConfig::OnlyImports
                } else {
                    return Err(l.error());
                };

                let list;
                syn::bracketed!(list in contents);
                let fields: Punctuated<syn::LitStr, Token![,]> =
                    list.parse_terminated(Parse::parse, Token![,])?;

                if contents.peek(Token![,]) {
                    contents.parse::<Token![,]>()?;
                }
                Ok(Opt::Async(
                    ctor(fields.iter().map(|s| s.value()).collect()),
                    span,
                ))
            }
        } else if l.peek(kw::ownership) {
            input.parse::<kw::ownership>()?;
            input.parse::<Token![:]>()?;
            let ownership = input.parse::<syn::Ident>()?;
            Ok(Opt::Ownership(match ownership.to_string().as_str() {
                "Owning" => Ownership::Owning,
                "Borrowing" => Ownership::Borrowing {
                    duplicate_if_necessary: {
                        let contents;
                        braced!(contents in input);
                        let field = contents.parse::<syn::Ident>()?;
                        match field.to_string().as_str() {
                            "duplicate_if_necessary" => {
                                contents.parse::<Token![:]>()?;
                                contents.parse::<syn::LitBool>()?.value
                            }
                            name => {
                                return Err(Error::new(
                                    field.span(),
                                    format!(
                                        "unrecognized `Ownership::Borrowing` field: `{name}`; \
                                         expected `duplicate_if_necessary`"
                                    ),
                                ));
                            }
                        }
                    },
                },
                name => {
                    return Err(Error::new(
                        ownership.span(),
                        format!(
                            "unrecognized ownership: `{name}`; \
                             expected `Owning` or `Borrowing`"
                        ),
                    ));
                }
            }))
        } else if l.peek(kw::trappable_error_type) {
            input.parse::<kw::trappable_error_type>()?;
            input.parse::<Token![:]>()?;
            let contents;
            let _lbrace = braced!(contents in input);
            let fields: Punctuated<_, Token![,]> =
                contents.parse_terminated(trappable_error_field_parse, Token![,])?;
            Ok(Opt::TrappableErrorType(Vec::from_iter(fields)))
        } else if l.peek(kw::interfaces) {
            input.parse::<kw::interfaces>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Interfaces(input.parse::<syn::LitStr>()?))
        } else if l.peek(kw::with) {
            input.parse::<kw::with>()?;
            input.parse::<Token![:]>()?;
            let contents;
            let _lbrace = braced!(contents in input);
            let fields: Punctuated<(String, String), Token![,]> =
                contents.parse_terminated(with_field_parse, Token![,])?;
            Ok(Opt::With(HashMap::from_iter(fields)))
        } else {
            Err(l.error())
        }
    }
}

fn trappable_error_field_parse(input: ParseStream<'_>) -> Result<TrappableError> {
    let wit_path = input.parse::<syn::LitStr>()?.value();
    input.parse::<Token![=>]>()?;
    let rust_type_name = input.parse::<syn::Path>()?.to_token_stream().to_string();
    Ok(TrappableError {
        wit_path,
        rust_type_name,
    })
}

fn with_field_parse(input: ParseStream<'_>) -> Result<(String, String)> {
    let interface = input.parse::<syn::LitStr>()?.value();
    input.parse::<Token![:]>()?;
    let start = input.span();
    let path = input.parse::<syn::Path>()?;

    // It's not possible for the segments of a path to be empty
    let span = start
        .join(path.segments.last().unwrap().ident.span())
        .unwrap_or(start);

    let mut buf = String::new();
    let append = |buf: &mut String, segment: syn::PathSegment| -> Result<()> {
        if segment.arguments != syn::PathArguments::None {
            return Err(Error::new(
                span,
                "Module path must not contain angles or parens",
            ));
        }

        buf.push_str(&segment.ident.to_string());

        Ok(())
    };

    if path.leading_colon.is_some() {
        buf.push_str("::");
    }

    let mut segments = path.segments.into_iter();

    if let Some(segment) = segments.next() {
        append(&mut buf, segment)?;
    }

    for segment in segments {
        buf.push_str("::");
        append(&mut buf, segment)?;
    }

    Ok((interface, buf))
}
