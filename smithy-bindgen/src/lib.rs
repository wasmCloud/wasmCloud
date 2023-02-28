///! smithy-bindgen macros
///!
use proc_macro2::Span;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, str::FromStr};
use syn::{
    bracketed, parse::Parse, parse::ParseStream, parse::Result, punctuated::Punctuated,
    spanned::Spanned, token, Error, LitStr, Token,
};
use weld_codegen::{
    config::{ModelSource, OutputFile},
    generators::{CodeGen, RustCodeGen},
    render::Renderer,
    sources_to_model,
    writer::Writer,
};

const BASE_MODEL_URL: &str = "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces";
const CORE_MODEL: &str = "core/wasmcloud-core.smithy";
const MODEL_MODEL: &str = "core/wasmcloud-model.smithy";

/// Generate code from a smithy IDL file.
///
/// ## Syntax
///
/// The first parameter of the `smithy_bindgen!` macro can take one of three forms.
/// The second parameter is the namespace used for code generation.
///
/// - one wasmcloud first-party interface  
///
///   The single-file parameter is a path relative to the wasmcloud interfaces git repo `wasmcloud/interfaces`
///
///   ```
///   # use smithy_bindgen::smithy_bindgen;
///   smithy_bindgen!("httpserver/httpserver.smithy", "org.wasmcloud.interfaces.httpserver");
///   ````
///
///   The above is shorthand for the following:
///   ```
///   # use smithy_bindgen::smithy_bindgen;
///   smithy_bindgen!({
///     url: "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces",
///     files: ["httpserver/httpserver.smithy"]
///   }, "org.wasmcloud.interfaces.httpserver" );
///   ```
///
/// - one Model Source
///
///   ```
///   # use smithy_bindgen::smithy_bindgen;
///   smithy_bindgen!({
///     path: "./tests/test-bindgen.smithy",
///   }, "org.example.interfaces.foo" );
///   ````
///
/// - array of Model Sources
///
///   ```
///   # use smithy_bindgen::smithy_bindgen;
///   smithy_bindgen!([
///     { path: "./tests/test-bindgen.smithy" },
///     { url: "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces/factorial/factorial.smithy" },
///   ], "org.example.interfaces.foo" );
///   ```
///
/// ## Model Source Specification
///
/// A model source contains a `url`, for http(s) downloads, or a `path`, for local fs access, that serves as a base, plus `files`, an optional list of file paths that are appended to the base to build complete url download paths and local file paths.
/// When joining the sub-paths from the `files` array, '/' is inserted or removed as needed, so that there is exactly one between the base and the sub-path.
/// `url` must begin with either 'http://' or 'https://'. If `path` is a relative fs path, it is relative to the folder containing `Cargo.toml`.
/// `files` may be omitted if the `url` or `path` contains the full path to the `.smithy` file.
///
/// All the following are (syntactically) valid model sources:
/// ```
/// { url: "https://example.com/interfaces/foo.smithy" }
/// { url: "https://example.com/interfaces", files: [ "foo.smithy", "bar.smithy" ]}
/// { path: "../interfaces/foo.smithy" }
/// { path: "../interfaces", files: ["foo.smithy", "bar.smithy"]}
/// ```
///
/// If a model source structure contains no url base and no path base,
/// the url for the github wasmcloud interface repo is used:
/// ```
/// url: "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces"
/// ```
///
/// Why would the code generator need to load more than one smithy file? So that interfaces can share common symbols for data structures. Most smithy interfaces already import symbols from the namespace `org.wasmcloud.model`, defined in `wasmcloud-model.smithy`.
/// The bindgen tool resolves all symbols by assembling an in-memory schema model from all the smithy sources and namespaces, then traversing through the in-memory model, generating code only for the schema elements in the namespace declared in the second parameter of `smithy_bindgen!`.
///
/// ## jsdelivr.net urls
///
/// `cdn.jsdelivr.net` mirrors open source github repositories.
/// The [url syntax](https://www.jsdelivr.com/?docs=gh) can optionally include
/// a github branch, tag, or commit sha.
///
/// ## Common files
///
/// Wasmcloud common model files are always automatically included when compiling models
/// (If you've used `codegen.toml` files, you may remember that they required all base models
/// to be specified explicitly.)
///
/// ## Namespace
///
/// Models may include symbols defined in other models via the `use` command.
/// Only the symbols defined in the namespace (`smithy_bindgen!`'s second parameter)
/// will be included in the generated code.
#[proc_macro]
pub fn smithy_bindgen(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let bindgen = syn::parse_macro_input!(input as BindgenConfig);
    generate_source(bindgen)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// parse sources into smithy ast model, then write 'namespace' to generated code
fn generate_source(bindgen: BindgenConfig) -> Result<proc_macro2::TokenStream> {
    let call_site = Span::call_site();
    let sources = bindgen
        .sources
        .into_iter()
        .map(SmithySource::into)
        .collect::<Vec<ModelSource>>();
    let mut w = Writer::default();
    let model = sources_to_model(&sources, &PathBuf::new(), 0).map_err(|e| {
        Error::new(
            call_site.span(),
            format!("cannot compile model sources: {}", e),
        )
    })?;
    let mut rust_gen = RustCodeGen::new(Some(&model));
    let output_config = OutputFile {
        namespace: Some(bindgen.namespace),
        ..Default::default()
    };
    let mut params = BTreeMap::<String, serde_json::Value>::default();
    params.insert("model".into(), atelier_json::model_to_json(&model));
    let mut renderer = Renderer::default();
    let bytes = rust_gen
        .init(Some(&model), &Default::default(), None, &mut renderer)
        .and_then(|_| rust_gen.generate_file(&mut w, &model, &output_config, &params))
        .map_err(|e| {
            Error::new(
                call_site.span(),
                format!("cannot generate rust source: {}", e),
            )
        })?;
    proc_macro2::TokenStream::from_str(&String::from_utf8_lossy(&bytes)).map_err(|e| {
        Error::new(
            call_site.span(),
            format!("cannot parse generated code: {}", e),
        )
    })
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SmithySource {
    url: Option<String>,
    path: Option<String>,
    files: Vec<String>,
}

/// internal struct used by smithy-bindgen
#[derive(Debug, Default, Serialize, Deserialize)]
struct BindgenConfig {
    pub sources: Vec<SmithySource>,
    pub namespace: String,
}

impl From<SmithySource> for ModelSource {
    fn from(source: SmithySource) -> Self {
        match (source.url, source.path) {
            (Some(url), _) => ModelSource::Url { url, files: source.files },
            (_, Some(path)) => ModelSource::Path { path: path.into(), files: source.files },
            _ => unreachable!(),
        }
    }
}

mod kw {
    syn::custom_keyword!(url);
    syn::custom_keyword!(path);
    syn::custom_keyword!(files);
}

enum Opt {
    Url(String),
    Path(String),
    Files(Vec<String>),
}

impl Parse for Opt {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let l = input.lookahead1();
        if l.peek(kw::url) {
            input.parse::<kw::url>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Url(input.parse::<LitStr>()?.value()))
        } else if l.peek(kw::path) {
            input.parse::<kw::path>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Path(input.parse::<LitStr>()?.value()))
        } else if l.peek(kw::files) {
            input.parse::<kw::files>()?;
            input.parse::<Token![:]>()?;
            let content;
            let _array = bracketed!(content in input);
            let files = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .map(|val| val.value())
                .collect();
            Ok(Opt::Files(files))
        } else {
            Err(l.error())
        }
    }
}

impl Parse for SmithySource {
    fn parse(input: ParseStream<'_>) -> syn::parse::Result<Self> {
        let call_site = Span::call_site();
        let mut source = SmithySource::default();
        let content;
        syn::braced!(content in input);
        let fields = Punctuated::<Opt, Token![,]>::parse_terminated(&content)?;
        for field in fields.into_pairs() {
            match field.into_value() {
                Opt::Url(s) => {
                    if source.url.is_some() {
                        return Err(Error::new(s.span(), "cannot specify second url"));
                    }
                    if source.path.is_some() {
                        return Err(Error::new(s.span(), "cannot specify path and url"));
                    }
                    source.url = Some(s)
                }
                Opt::Path(s) => {
                    if source.path.is_some() {
                        return Err(Error::new(s.span(), "cannot specify second path"));
                    }
                    if source.url.is_some() {
                        return Err(Error::new(s.span(), "cannot specify path and url"));
                    }
                    source.path = Some(s)
                }
                Opt::Files(val) => source.files = val,
            }
        }
        if !(!source.files.is_empty()
            || (source.url.is_some() && source.url.as_ref().unwrap().ends_with(".smithy"))
            || (source.path.is_some() && source.path.as_ref().unwrap().ends_with(".smithy")))
        {
            return Err(Error::new(
                call_site.span(),
                "There must be at least one .smithy file",
            ));
        }
        if source.url.is_none() && source.path.is_none() {
            source.url = Some(BASE_MODEL_URL.to_string());
        }
        Ok(source)
    }
}

impl Parse for BindgenConfig {
    fn parse(input: ParseStream<'_>) -> syn::parse::Result<Self> {
        let call_site = Span::call_site();
        let mut sources;

        let l = input.lookahead1();
        if l.peek(token::Brace) {
            // one source
            let source = input.parse::<SmithySource>()?;
            sources = vec![source];
        } else if l.peek(token::Bracket) {
            // list of sources
            let content;
            syn::bracketed!(content in input);
            sources = Punctuated::<SmithySource, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();
        } else if l.peek(LitStr) {
            // shorthand for wasmcloud default url
            let one_file = input.parse::<LitStr>()?;
            sources = vec![SmithySource {
                url: Some(BASE_MODEL_URL.into()),
                path: None,
                files: vec![
                    "core/wasmcloud-core.smithy".into(),
                    "core/wasmcloud-model.smithy".into(),
                    one_file.value(),
                ],
            }];
        } else {
            return Err(Error::new(
                call_site.span(),
                "expected quoted path, or model source { url or path: ...,  files: ,.. }, or list of model sources [...]"
            ));
        }
        input.parse::<Token![,]>()?;
        let namespace = input.parse::<LitStr>()?.value();

        // append base models if either are missing
        let has_core = sources.iter().any(|s| {
            (s.url.is_some() && s.url.as_ref().unwrap().ends_with(CORE_MODEL))
                || s.files.iter().any(|s| s.ends_with(CORE_MODEL))
        });
        let has_model = sources.iter().any(|s| {
            (s.url.is_some() && s.url.as_ref().unwrap().ends_with(MODEL_MODEL))
                || s.files.iter().any(|s| s.ends_with(MODEL_MODEL))
        });
        if !has_core || !has_model {
            sources.push(SmithySource {
                url: Some(BASE_MODEL_URL.into()),
                files: match (has_core, has_model) {
                    (false, false) => vec![CORE_MODEL.into(), MODEL_MODEL.into()],
                    (false, true) => vec![CORE_MODEL.into()],
                    (true, false) => vec![MODEL_MODEL.into()],
                    _ => unreachable!(),
                },
                path: None,
            });
        }
        Ok(BindgenConfig { sources, namespace })
    }
}
