use crate::{
    model::{expect_member, CommentKind, ModelIndex},
    writer::ToBytes,
};
use atelier_core::{
    model::{Model, ShapeID},
    Version,
};
use bytes::{Bytes, BytesMut};
use std::fmt;

//pub mod codegen_as;
//pub mod codegen_go;
pub mod codegen_rust;
pub mod error;
pub mod model;
/// utility for running 'rustfmt'
#[cfg(not(target_arch = "wasm32"))]
pub mod rustfmt;
pub mod writer;

use error::{Error, Result};

/// Languages available for output generation
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum OutputLanguage {
    Rust,
}

impl Default for OutputLanguage {
    fn default() -> OutputLanguage {
        OutputLanguage::Rust
    }
}

impl std::str::FromStr for OutputLanguage {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "rust" => Ok(OutputLanguage::Rust),
            _ => panic!("Invalid output language: {}", s),
        }
    }
}

impl fmt::Display for OutputLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                OutputLanguage::Rust => "rust",
            }
        )
    }
}

//pub enum MessageDirection {
//    ActorToActor,
//    ActorToProvider,
//    ProviderToActor,
//}

/// Read model file(s)
#[cfg(not(target_arch = "wasm32"))]
pub fn load_model(paths: &[std::path::PathBuf]) -> Result<Model> {
    let mut model = atelier_core::model::Model::new(Version::V10);
    for p in paths {
        if !p.is_file() {
            return Err(Error::MissingFile(format!("{}", p.display())));
        }
        let text = std::fs::read_to_string(&p)?;
        let m = atelier_smithy::parse_model(&text)
            .map_err(|e| Error::Model(format!("parsing model from '{}': {:?}", p.display(), e)))?;
        model
            .merge(m)
            .map_err(|e| Error::Model(format!("merging model from '{}': {:?}", p.display(), e)))?;
    }
    Ok(model)
}

pub mod strings {

    /// convert a string to a module name
    pub use inflector::cases::{
        camelcase::to_camel_case, pascalcase::to_pascal_case, snakecase::to_snake_case,
    };

    /// remove leading and trailing quotes, if present
    pub fn unquote(s: &str) -> &str {
        match s.strip_prefix('"') {
            Some(start) => match start.strip_suffix('"') {
                Some(both) => both,
                None => s,
            },
            None => s,
        }
    }
}

/// A Codegen is used to generate source for a Smithy Model
/// The generator will invoke these functions (in order)
/// - init()
/// - write_source_file_header()
/// - declare_types()
/// - write_services()
/// - finalize()
///
/// Each of these methods receives a reference to a ModelIndex, which provides
/// sorted collections of each of the model types.
pub trait CodeGen {
    /// Generate file
    fn codegen(&mut self, model: &Model) -> std::result::Result<Bytes, Error>
    where
        Self: Sized,
    {
        let ix = ModelIndex::build(model);
        // Unions are not supported because msgpack doesn't know how to serialize them
        expect_empty!(ix.unions, "Unions are not supported");
        // might support these in the future, but not yet
        expect_empty!(ix.resources, "Resources are not supported");
        // indicates a model error - probably typo or forgot to include a definition file
        expect_empty!(ix.unresolved, "types could not be determined");

        self.init(&ix)?;
        self.write_source_file_header(&ix)?;
        self.declare_types(&ix)?;
        self.write_services(&ix)?;
        self.finalize()
    }

    /// Perform any initialization required prior to code generation
    /// `model` may be used to check model metadata
    fn init(&mut self, ix: &ModelIndex) -> std::result::Result<(), Error>;

    /// generate the source file header
    fn write_source_file_header(&mut self, ix: &ModelIndex) -> std::result::Result<(), Error>;

    /// Write declarations for simple types, maps, and structures
    fn declare_types(&mut self, ix: &ModelIndex) -> std::result::Result<(), Error>;

    /// Write service declarations and implementation stubs
    fn write_services(&mut self, ix: &ModelIndex) -> std::result::Result<(), Error>;

    /// Complete generation and return the output bytes
    fn finalize(&mut self) -> std::result::Result<Bytes, Error> {
        Ok(self.take().freeze())
    }

    /// Write documentation for item
    fn write_documentation(&mut self, _id: &ShapeID, text: &str) {
        for line in text.split('\n') {
            // remove whitespace from end of line
            let line = line.trim_end_matches(|c| c == '\r' || c == ' ' || c == '\t');
            self.write_comment(CommentKind::Documentation, line);
        }
    }

    /// Writes single-line comment beginning with '// '
    /// Can be overridden if more specific kinds are needed
    fn write_comment(&mut self, _kind: CommentKind, line: &str) {
        self.write(b"// ");
        self.write(line);
        self.write(b"\n");
    }

    /// Writes info the the current output writer
    fn write<B: ToBytes>(&mut self, bytes: B);

    /// Returns the current buffer, zeroing out self
    fn take(&mut self) -> BytesMut;

    /// returns file extension of source files for this language
    fn get_file_extension(&self) -> &'static str;

    /// apply proper casing to a type name: any trait, struct, or user-defined (non-primitive) type
    /// The default implementation uses UpperPascalCase
    fn to_type_name(&self, s: &str) -> String {
        crate::strings::to_pascal_case(s)
    }

    /// Convert case to language-applicable method name
    /// Default implementation uses snake_case
    fn to_method_name(&self, id: &ShapeID) -> String {
        crate::strings::to_snake_case(&id.shape_name().to_string())
    }

    /// Convert case for structure field name
    /// Default implementation uses snake_case
    fn to_field_name(&self, id: &ShapeID) -> std::result::Result<String, Error> {
        Ok(crate::strings::to_snake_case(&expect_member(id)?))
    }

    /// The operation name used in dispatch, from method
    /// The default implementation is provided and should not be overridden
    fn op_dispatch_name(&self, id: &ShapeID) -> String {
        crate::strings::to_pascal_case(&id.shape_name().to_string())
    }

    /// The full operation name with service prefix, with surrounding quotes
    /// The default implementation is provided and should not be overridden
    fn full_dispatch_name(&self, service_id: &ShapeID, method_id: &ShapeID) -> String {
        format!(
            "\"{}.{}\"",
            &self.to_type_name(&service_id.shape_name().to_string()),
            &self.op_dispatch_name(method_id)
        )
    }
}
