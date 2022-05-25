use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use atelier_core::model::Model;

use crate::{
    config::{LanguageConfig, OutputFile, OutputLanguage},
    error::Result,
    format::SourceFormatter,
    gen::{to_json, CodeGen},
    render::Renderer,
    Bytes, Error, JsonValue, ParamMap,
};

// default page name for doc template
const DOC_TEMPLATE: &str = "namespace_doc";

/// Default templates
pub const HTML_TEMPLATES: &[(&str, &str)] = &[
    ("page_base", include_str!("../templates/html/page_base.hbs")),
    (
        "namespace_doc",
        include_str!("../templates/html/namespace_doc.hbs"),
    ),
];

#[derive(Debug, Default)]
pub(crate) struct DocGen {}

impl CodeGen for DocGen {
    fn output_language(&self) -> OutputLanguage {
        OutputLanguage::Html
    }

    /// Initialize code generator and renderer for language output.j
    /// This hook is called before any code is generated and can be used to initialize code generator
    /// and/or perform additional processing before output files are created.
    fn init(
        &mut self,
        model: Option<&Model>,
        lc: &LanguageConfig,
        output_dir: &Path,
        renderer: &mut Renderer,
    ) -> std::result::Result<(), Error> {
        let model = match model {
            None => return Ok(()),
            Some(model) => model,
        };
        for t in HTML_TEMPLATES.iter() {
            renderer.add_template(*t)?;
        }
        let mut params: BTreeMap<String, JsonValue> = to_json(&lc.parameters)?;
        let json_model = atelier_json::model_to_json(model);
        params.insert("model".to_string(), json_model);

        let minified = match params.get("minified") {
            Some(JsonValue::Bool(b)) => *b,
            _ => false,
        };
        params.insert("minified".to_string(), JsonValue::Bool(minified));
        let doc_template = match params.get("doc_template") {
            Some(JsonValue::String(s)) => s.clone(),
            _ => DOC_TEMPLATE.to_string(),
        };

        // renderer is already initialized with "model" as the json-ast model,
        // but Model has a more convenient way to get namespaces.
        // Get list of namespaces from top level shapes, using BTreeSet to remove duplicates
        let namespaces = model
            .namespaces()
            .iter()
            .map(|id| id.to_string())
            .collect::<BTreeSet<String>>();

        std::fs::create_dir_all(&output_dir).map_err(|e| {
            Error::Io(format!(
                "creating directory {}: {}",
                output_dir.display(),
                e
            ))
        })?;

        for ns in namespaces.iter() {
            let output_file =
                output_dir.join(format!("{}.html", crate::strings::to_snake_case(ns)));

            let mut out = std::fs::File::create(&output_file).map_err(|e| {
                Error::Io(format!(
                    "writing output file {}: {}",
                    output_file.display(),
                    e
                ))
            })?;
            params.insert("namespace".to_string(), JsonValue::String(ns.clone()));
            params.insert("title".to_string(), JsonValue::String(ns.clone()));
            renderer.render(&doc_template, &params, &mut out)?;
        }

        Ok(())
    }

    /// DocGen doesn't do per-file generation so this is a no-op
    fn generate_file(
        &mut self,
        _model: &Model,
        _file_config: &OutputFile,
        _params: &ParamMap,
    ) -> Result<Bytes> {
        Ok(Bytes::new())
    }

    // never called
    fn get_file_extension(&self) -> &'static str {
        "html"
    }

    fn to_method_name_case(&self, name: &str) -> String {
        name.into()
    }
    fn to_field_name_case(&self, name: &str) -> String {
        name.into()
    }
    fn to_type_name_case(&self, name: &str) -> String {
        name.into()
    }
    fn source_formatter(&self, _: Vec<String>) -> Result<Box<dyn SourceFormatter>> {
        Ok(Box::new(crate::format::NullFormatter::default()))
    }
}
