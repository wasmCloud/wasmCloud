//! Code generation
//!
use std::str::FromStr;

use atelier_core::model::{Identifier, NamespaceID, ShapeID};
pub use handlebars::RenderError;
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, Output, RenderContext, ScopedJson,
};
use serde::Serialize;
use serde_json::Value;

use crate::{strings, JsonMap, JsonValue};

// these defaults can be overridden by the config file

const DOCUMENTATION_TRAIT: &str = "smithy.api#documentation";
const TRAIT_TRAIT: &str = "smithy.api#trait";

/// All smithy simple shapes
const SIMPLE_SHAPES: &[&str] = &[
    "string",
    "integer",
    "long",
    "blob",
    "boolean",
    "byte",
    "double",
    "float",
    "short",
    "bigDecimal",
    "bigInteger",
    "timestamp",
    "document",
];

/// simple shapes + list, map, union
const BASIC_TYPES: &[&str] = &[
    "string",
    "integer",
    "long",
    "blob",
    "boolean",
    "byte",
    "double",
    "float",
    "short",
    "list",
    "map",
    "union",
    "bigDecimal",
    "bigInteger",
    "timestamp",
    "document",
];

/// Pairing of template name and contents
///
pub type Template<'template> = (&'template str, &'template str);

#[derive(Default, Debug)]
pub struct RenderConfig<'render> {
    /// Templates to be loaded for renderer. List of template name, data
    pub templates: Vec<Template<'render>>,
    /// Whether parser is in strict mode:
    ///   If true, a variable used in template that is undefined would raise an error
    ///   if false, an undefined variable would evaluate to 'falsey'
    pub strict_mode: bool,
}

/// HBTemplate processor for code generation
pub struct Renderer<'gen> {
    /// Handlebars processor
    hb: Handlebars<'gen>,
}

impl<'gen> Default for Renderer<'gen> {
    fn default() -> Self {
        // unwrap ok because only error condition occurs with templates, and default has none.
        Self::init(&RenderConfig::default()).unwrap()
    }
}

impl<'gen> Renderer<'gen> {
    /// Initialize handlebars template processor.
    pub fn init(config: &RenderConfig) -> Result<Self, crate::Error> {
        let mut hb = Handlebars::new();
        // don't use strict mode because
        // it's easier in templates to use if we allow undefined ~= false-y
        hb.set_strict_mode(config.strict_mode);
        hb.register_escape_fn(handlebars::no_escape); //html escaping is the default and cause issue0

        // add common helpers and templates
        add_base_helpers(&mut hb);
        for t in &config.templates {
            hb.register_template_string(t.0, t.1)?;
        }

        Ok(Self { hb })
    }

    /// Adds template to internal dictionary
    pub fn add_template(&mut self, template: Template) -> Result<(), crate::Error> {
        self.hb.register_template_string(template.0, template.1)?;
        Ok(())
    }

    /// render a template without registering it
    pub fn render_template<T>(&self, template: &str, data: &T) -> Result<String, crate::Error>
    where
        T: Serialize,
    {
        let rendered = self.hb.render_template(template, data)?;
        Ok(rendered)
    }

    /// Render a named template
    pub fn render<T, W>(
        &self,
        template_name: &str,
        data: &T,
        writer: &mut W,
    ) -> Result<(), crate::Error>
    where
        T: Serialize,
        W: std::io::Write,
    {
        self.hb.render_to_write(template_name, data, writer)?;
        Ok(())
    }
}

fn arg_as_string<'reg, 'rc>(
    h: &'reg Helper<'reg, 'rc>,
    n: usize,
    tag: &str,
) -> Result<&'rc str, RenderError> {
    // get first arg as string
    h.param(n)
        .ok_or_else(|| RenderError::new(format!("missing string param after {}", tag)))?
        .value()
        .as_str()
        .ok_or_else(|| {
            RenderError::new(format!(
                "{} expects string param, not {:?}",
                tag,
                h.param(n).unwrap().value()
            ))
        })
}

fn arg_as_obj<'reg, 'rc>(
    h: &'reg Helper<'reg, 'rc>,
    n: usize,
    tag: &str,
) -> Result<&'rc serde_json::Map<String, serde_json::Value>, RenderError> {
    // get first arg as string
    h.param(n)
        .ok_or_else(|| RenderError::new(format!("missing object param after {}", tag)))?
        .value()
        .as_object()
        .ok_or_else(|| {
            RenderError::new(format!(
                "{} expects object param, not {:?}",
                tag,
                h.param(n).unwrap().value()
            ))
        })
}

fn arg_as_array<'reg, 'rc>(
    h: &'reg Helper<'reg, 'rc>,
    n: usize,
    tag: &str,
) -> Result<&'rc Vec<serde_json::Value>, RenderError> {
    // get first arg as string
    h.param(n)
        .ok_or_else(|| RenderError::new(format!("missing array param after {}", tag)))?
        .value()
        .as_array()
        .ok_or_else(|| {
            RenderError::new(format!(
                "{} expects array param, not {:?}",
                tag,
                h.param(n).unwrap().value()
            ))
        })
}

#[derive(Clone, Copy)]
struct ShapeHelper {}

/// Convert map iterator into Vec of sorted shapes, adding the map's key as field _key to each item
fn to_sorted_array<S: AsRef<str>>(mut shapes: Vec<(S, &Value)>) -> JsonValue {
    // case-insensitive, numeric-aware sort
    shapes.sort_unstable_by(|a, b| {
        lexical_sort::natural_lexical_only_alnum_cmp(a.0.as_ref(), b.0.as_ref())
    });

    let shapes = shapes
        .into_iter()
        .map(|(k, v)| (k.as_ref().to_string(), v.as_object().unwrap().clone()))
        .map(|(k, mut v)| {
            v.insert("_key".to_string(), serde_json::Value::String(k));
            serde_json::Value::Object(v)
        })
        .collect::<Vec<Value>>();
    Value::Array(shapes)
}

impl HelperDef for ShapeHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        let shape_kind = arg_as_string(h, 0, "filter_shapes")?.to_string();
        let arr = arg_as_array(h, 1, "filter_shapes")?;

        // filter by shape
        let shapes = arr
            .iter ()
            .filter(|v| {
                matches!(v.get("type"), Some(serde_json::Value::String(kind))
                    if (&shape_kind == "simple" && SIMPLE_SHAPES.contains(&kind.as_str()) && !val_is_trait(v))
                     || (&shape_kind == "types" && BASIC_TYPES.contains(&kind.as_str()) && !val_is_trait(v))
                        || (&shape_kind == "trait" && val_is_trait(v))
                        || (&shape_kind != "trait" && &shape_kind == kind && !val_is_trait(v))
                )
            })
            .cloned()
            .collect::<Vec<Value>>();
        Ok(ScopedJson::Derived(Value::Array(shapes)))
    }
}

#[derive(Clone, Copy)]
struct NamespaceHelper {}

impl HelperDef for NamespaceHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        let namespace = arg_as_string(h, 0, "filter_namespace")?;
        let namespace = NamespaceID::from_str(namespace)
            .map_err(|e| RenderError::new(&format!("invalid namespace {}", e)))?;
        let obj = arg_as_obj(h, 1, "filter_namespace")?;

        let shapes = obj
            .iter()
            .filter_map(|(k, v)| match ShapeID::from_str(k) {
                Ok(id) => Some((id, v)),
                _ => None,
            })
            .filter(|(id, _)| id.namespace() == &namespace)
            .map(|(id, v)| (id.to_string(), v))
            .collect::<Vec<(String, &Value)>>();
        Ok(ScopedJson::Derived(to_sorted_array(shapes)))
    }
}

#[derive(Clone, Copy)]
struct SimpleTypeHelper {}

impl HelperDef for SimpleTypeHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        let type_name = arg_as_string(h, 0, "is_simple")?;
        Ok(ScopedJson::Derived(serde_json::Value::Bool(
            SIMPLE_SHAPES.contains(&type_name),
        )))
    }
}

#[derive(Clone, Copy)]
struct DocHelper {}

impl HelperDef for DocHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        let mut doc = String::new();
        let shape_props = arg_as_obj(h, 0, "doc")?;
        if let Some(JsonValue::Object(traits)) = shape_props.get("traits") {
            if let Some(JsonValue::String(doc_value)) = traits.get(DOCUMENTATION_TRAIT) {
                doc = doc_value.clone();
                // TODO: should convert markdown to html!
            }
        }
        Ok(ScopedJson::Derived(serde_json::Value::String(doc)))
    }
}

/*
#[derive(Clone, Copy)]
struct TypeHelper {}

/// pretty-print type names
impl HelperDef for TypeHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<Option<ScopedJson<'reg, 'rc>>, RenderError> {
        let typ = arg_as_string(h, 0, "typ")?;

        // strip off smithy.api since it makes everything too verbose
        // (unwrap here because (per smithy json-ast spec) model was created with all absolute shape ids
        let sid = ShapeID::from_str(typ).unwrap();
        if sid.namespace() == &NamespaceID::new_unchecked("smithy.api") {
            return Ok(Some(ScopedJson::Derived(serde_json::Value::String(
                sid.shape_name().to_string(),
            ))));
        }

        // if a namespace param was provided, strip off that if this type is local to that namespace
        if let Ok(ns) = arg_as_string(h, 1, "typ") {
            if &sid.namespace().to_string() == ns {
                return Ok(Some(ScopedJson::Derived(serde_json::Value::String(
                    sid.shape_name().to_string(),
                ))));
            }
        }

        // otherwise return as-is
        Ok(Some(ScopedJson::Derived(serde_json::Value::String(
            typ.to_string(),
        ))))
    }
}
 */

/// Returns true if the shape is a trait
fn map_is_trait(shape: &JsonMap) -> bool {
    if let Some(JsonValue::Object(traits)) = shape.get("traits") {
        traits.get(TRAIT_TRAIT).is_some()
    } else {
        false
    }
}

/// Returns true if the shape is a trait
fn val_is_trait(shape: &JsonValue) -> bool {
    if let Some(JsonValue::Object(traits)) = shape.get("traits") {
        traits.get(TRAIT_TRAIT).is_some()
    } else {
        false
    }
}

#[derive(Clone, Copy)]
struct TraitsHelper {}

/// Returns a copy of the shape's traits without documentation trait
impl HelperDef for TraitsHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        let mut traits_no_doc = JsonMap::new();

        let shape_props = arg_as_obj(h, 0, "traits")?;
        if let Some(JsonValue::Object(traits)) = shape_props.get("traits") {
            for (k, v) in traits.iter() {
                if k != DOCUMENTATION_TRAIT && k != TRAIT_TRAIT {
                    traits_no_doc.insert(k.clone(), v.clone());
                }
            }
        }
        Ok(ScopedJson::Derived(serde_json::Value::Object(
            traits_no_doc,
        )))
    }
}

/*
fn to_href_link(id: &str) -> String {
    let id =
        ShapeID::from_str(id).map_err(|e| RenderError::new(&format!("invalid shape id {}", e)))?;
    let ns = strings::to_camel_case(id.namespace().to_string());
    format!(
        "<a href=\"../{}.html#{}\">{}</a>",
        ns,
        id.shape_name().to_string(),
        id
    )
}:w

 */

#[derive(Clone, Copy)]
struct IsTraitHelper {}

/// Returns true if the shape is a trait
impl HelperDef for IsTraitHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'reg, 'rc>,
        _reg: &'reg Handlebars<'reg>,
        _ctx: &'rc Context,
        _rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        let shape = arg_as_obj(h, 0, "is_trait")?;
        Ok(ScopedJson::Derived(serde_json::Value::Bool(map_is_trait(
            shape,
        ))))
    }
}

/// Add template helpers functions
fn add_base_helpers(hb: &mut Handlebars) {
    // "shapes" filters a shape list for the shape kind
    //   `shapes kind`      - uses 'this' for the list of shapes; should be called inside an #each block
    //   `shapes kind list` - uses the provided 'list' object, assumed to be a dict of shapes in json-ast format
    hb.register_helper("filter_shapes", Box::new(ShapeHelper {}));

    // "namespaces" filters a shape list for shapes in the namespace
    //   `namespaces ns`      - finds shapes in `this` that are in namespace ns
    //   `namespaces ns list` - finds shapes in `list` in namespace ns
    hb.register_helper("filter_namespace", Box::new(NamespaceHelper {}));

    // "is_simple" returns true if the type parameter is one of the simple types
    hb.register_helper("is_simple", Box::new(SimpleTypeHelper {}));

    // "doc" extracts documentation for the object (or item)
    hb.register_helper("doc", Box::new(DocHelper {}));

    // "traits" returns object's traits without documentation
    hb.register_helper("traits", Box::new(TraitsHelper {}));

    // "traits" returns object's traits without documentation
    hb.register_helper("is_trait", Box::new(IsTraitHelper {}));

    //
    // extract the namespace part of a ShapeID
    //
    hb.register_helper(
        "namespace_name",
        Box::new(
            |h: &Helper,
             _r: &Handlebars,
             _: &Context,
             _rc: &mut RenderContext,
             out: &mut dyn Output|
             -> HelperResult {
                // get first arg as string
                let id = arg_as_string(h, 0, "namespace")?;
                let id = ShapeID::from_str(id).map_err(|e| {
                    RenderError::new(&format!("invalid shape id {} for namespace_name", e))
                })?;
                out.write(&id.namespace().to_string())?;
                Ok(())
            },
        ),
    );

    hb.register_helper(
        "typ",
        Box::new(
            |h: &Helper,
             _r: &Handlebars,
             _: &Context,
             _rc: &mut RenderContext,
             out: &mut dyn Output|
             -> HelperResult {
                let typ = arg_as_string(h, 0, "typ")?;
                let sid = ShapeID::from_str(typ).unwrap();
                // (unwrap here ok because (per smithy json-ast spec) model was created with all absolute shape ids
                let sid_ns = sid.namespace();

                let link: String = if sid_ns == &NamespaceID::new_unchecked("smithy.api") {
                    // If it's in smithy.api, just use shape name since smithy.api makes it too verbose
                    sid.shape_name().to_string()
                } else {
                    match arg_as_string(h, 1, "typ") {
                        // If it's local to this file (namespace matches namespace parameter), strip it off and use local href
                        Ok(ns) if sid_ns.to_string() == ns => {
                            let id_shape = sid.shape_name().to_string();
                            format!(
                                "<a href=\"#{}\">{}</a>",
                                &strings::to_snake_case(&id_shape),
                                &id_shape,
                            )
                        }
                        _ => format!(
                            "<a href=\"./{}.html#{}\">{}</a>",
                            &strings::to_snake_case(&sid_ns.to_string()),
                            &strings::to_snake_case(&sid.shape_name().to_string()),
                            sid
                        ),
                    }
                };
                out.write(&link)?;
                Ok(())
            },
        ),
    );

    //
    // extract the shape-name part of a ShapeID
    //
    hb.register_helper(
        "shape_name",
        Box::new(
            |h: &Helper,
             _r: &Handlebars,
             _: &Context,
             _rc: &mut RenderContext,
             out: &mut dyn Output|
             -> HelperResult {
                let id = arg_as_string(h, 0, "shape_name")?;
                let id = ShapeID::from_str(id).map_err(|e| {
                    RenderError::new(&format!("invalid shape id {} for shape_name", e))
                })?;
                out.write(&id.shape_name().to_string())?;
                Ok(())
            },
        ),
    );

    //
    // extract the member name of the shape, if any
    //
    hb.register_helper(
        "member_name",
        Box::new(
            |h: &Helper,
             _r: &Handlebars,
             _: &Context,
             _rc: &mut RenderContext,
             out: &mut dyn Output|
             -> HelperResult {
                let id = arg_as_string(h, 0, "member_name")?;
                let id = Identifier::from_str(id).map_err(|e| {
                    RenderError::new(&format!("invalid member id {} for member_name", e))
                })?;
                out.write(&id.to_string())?;
                Ok(())
            },
        ),
    );

    //
    // to_pascal_case
    //
    hb.register_helper(
        "to_pascal_case",
        Box::new(
            |h: &Helper,
             _r: &Handlebars,
             _: &Context,
             _rc: &mut RenderContext,
             out: &mut dyn Output|
             -> HelperResult {
                let id = arg_as_string(h, 0, "to_pascal_case")?;
                out.write(&strings::to_pascal_case(id))?;
                Ok(())
            },
        ),
    );

    //
    // to_snake_case
    //
    hb.register_helper(
        "to_snake_case",
        Box::new(
            |h: &Helper,
             _r: &Handlebars,
             _: &Context,
             _rc: &mut RenderContext,
             out: &mut dyn Output|
             -> HelperResult {
                let id = arg_as_string(h, 0, "to_snake_case")?;
                out.write(&strings::to_snake_case(id))?;
                Ok(())
            },
        ),
    );
}
