// wasmcloud-model.smithy
// Base types and traits used for wasmcloud models
//

// Tell the code generator how to reference symbols defined in this namespace
metadata package = [ { namespace: "org.wasmcloud.model", crate: "wasmbus_rpc::model" } ]

namespace org.wasmcloud.model

// Definitions for api modeling and code generation
// Traits declared here _may_ be used for code generation.
// No services should be defined here.

/// Overrides for serializer & deserializer
@trait(selector: "member")
structure serialization {
    /// (optional setting) Override field name when serializing and deserializing
    /// By default, (when `name` not specified) is the exact declared name without
    /// casing transformations. This setting does not affect the field name
    /// produced in code generation, which is always lanaguage-idiomatic
    name: String,
}

/// The unsignedInt trait indicates that one of the number types is unsigned
@trait(selector: "long,integer,short,byte")
@range(min:0)
structure unsignedInt { }


/// A non-empty string (minimum length 1)
@trait(selector: "string")
@length(min:1)
string nonEmptyString

/// This trait doesn't have any functional impact on codegen. It is simply
/// to document that the defined type is a synonym, and to silence
/// the default validator that prints a notice for synonyms with no traits.
@trait
structure synonym{}

/// signed 64-bit int
@synonym
long I64

/// unsigned 64-bit int
@unsignedInt
long U64

/// signed 32-bit int
@synonym
integer I32

/// unsigned 32-bit int
@unsignedInt
integer U32

/// signed 16-bit int
@synonym
short I16

/// unsigned 16-bit int
@unsignedInt
short U16

/// signed byte
@synonym
byte I8

/// unsigned byte
@unsignedInt
byte U8

/// Rust codegen traits
@trait(selector: "structure")
structure codegenRust {
    /// Instructs rust codegen to add `#[derive(Default)]` (default false)
    deriveDefault: Boolean,
}

/// a protocol defines the semantics
/// of how a client and server communicate.
@protocolDefinition
@trait(selector: "service")
structure wasmbus {
    /// capability id such as "wasmbus:httpserver"
    /// always required for providerReceive, but optional for actorReceive
    contractId: CapabilityContractId,
    /// indicates this service's operations are handled by an actor (default false)
    actorReceive: Boolean,
    /// indicates this service's operations are handled by an provider (default false)
    providerReceive: Boolean,
}

/// data sent via wasmbus
/// This trait is required for all messages sent via wasmbus
@trait(selector: "simpleType,list,set,map,structure")
@codegenRust( deriveDefault: true )
structure wasmbusData {}

/// Capability contract id, e.g. 'wasmcloud:httpserver'
@nonEmptyString
string CapabilityContractId
