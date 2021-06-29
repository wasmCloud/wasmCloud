namespace org.wasmcloud.model.v0

// Note: this file has moved into the wasmcloud/models crate

/// definitions for api modeling
/// These are modifications to the basic data model

/// The unsignedInt trait indicates that one of the number types is unsigned
@trait(selector: "long,integer,short,byte")
@range(min:0)
structure unsignedInt { }


/// A non-empty string
@trait(selector: "string")
@length(min:1)
string nonEmptyString


/// Overrides for serializer & deserializer
@trait(selector: "member")
structure serialize {
    /// (optional setting) Override name for field when serializing and deserializing
    /// By default, (when `rename` not specified) is the exact declared name without
    /// casing transformations. This setting does not affect the field name
    /// produced in code generation, which can vary by language.
    rename: String,
}

/// This trait doesn't have any functional impact on codegen. It is simply
/// to document that the defined type is a synonym, and to silence
/// the default validator that prints a notice for synonyms with no traits.
@trait
structure synonym{}

// signed 64-bit int
@synonym
long I64

// unsigned 64-bit int
@unsignedInt
long U64

// signed 32-bit int
@synonym
integer I32

// unsigned 32-bit int
@unsignedInt
integer U32

// signed 16-bit int
@synonym
short I16

// unsigned 16-bit int
@unsignedInt
short U16

// signed byte
@synonym
byte I8

// unsigned byte
@unsignedInt
byte U8

