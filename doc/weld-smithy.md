# Weld-Smithy Guide

Weld's use of Smithy closely follows the [Smithy IDL specification](https://awslabs.github.io/smithy/1.0/spec/core/idl.html). This document is intended to be a quick reference to the major features of Smithy, and includes some conventions on the way we use Smithy with `weld`.

__Contents__

- [Models](#models)
- [Data types](#data-types)
- [Structures](#structures)  
- [Operations](#operations)
- [Documentation](#documentation)


## Models

We author models in Smithy IDL files, with a `.smithy` extension. An IDL that defines any shapes must have a namespace declaration, 

A [Semantic Model](https://awslabs.github.io/smithy/1.0/spec/core/model.html#the-semantic-model) is built from multiple IDL files and/or json (AST model) files, and can contain multiple namespaces. To fully lint or validate a model, or to generate code from a semantic model, you need to specify paths to all dependencies in all namespaces used by the model. These dependencies are specified either on the `weld` command line, or in a [`codegen.toml`](./codegen-toml.md) configuration file, usually located in the project root folder.


## Data Types

In Smithy, data types are called [shapes](https://awslabs.github.io/smithy/1.0/spec/core/model.html#shapes).

### Supported Shapes

- simple shapes
  - `Byte`, `Short`, `Integer`, `Long`
  - `Float`, `Double`
  - `Boolean`
  - `String`
  - `Blob`
  - `Timestamp` (* partially supported, see below)

- aggregate shaptes
  - `list` [List](https://awslabs.github.io/smithy/1.0/spec/core/model.html#list) (array of any other type)
  - `set` [Set](https://awslabs.github.io/smithy/1.0/spec/core/model.html#set) (set of unique values)
  - `map` [Map](https://awslabs.github.io/smithy/1.0/spec/core/model.html#map) map of key-type to value-type)
  - `structure` [Structure](https://awslabs.github.io/smithy/1.0/spec/core/model.html#structure)
  - `service` [Service](https://awslabs.github.io/smithy/1.0/spec/core/model.html#service) a collection of operations
  - `operation` [Operation](https://awslabs.github.io/smithy/1.0/spec/core/model.html#operation) a function
    
The following Smithy shapes are __partially supported__: `set`, `Timestamp`

The following Smithy shapes are __not supported__ (yet): `BigInteger`, `BigDecimal`, `union`, `document`. `resource`

### Integer types

Smithy's primitive integer types (Byte, Short, Integer, Long) are signed.
The namespace `org.wasmcloud.model` defines unsigned types: (`U8`,`U16`,`U32`,`U64`)
and, for consistency, aliases (`I8`,`I16`,`I32`,`I64`) for the signed primitive types.
The unsigned types have the trait `@unsignedInt`, which has the trait `@limit(min:0)`.

The `@unsignedInt` trait causes the weld code generator to generate unsigned data types in languages that support them.


### Timestamp

Timestamp is currently supported only in Rust, but will be added to all supported output languages. A timestamp is represented approximately like this:

```rust
struct Timestamp {
  sec: u64,   // seconds since unix epoch in UTC (also called unix time)
  nano: u32,  // nanoseconds since the beginning of the last whole second
}
```

SDK Client libraries in supported languages will include functions for converting between a Timestamp and RFC3339 strings.

### Maps

Due to the way we use msgpack, map key types are limited to String.


## Structures

Structures are just like the structures in your favorite programming languages.

```text
/// Documentation for my structure
structure Point {
    x: Integer,
    y: Integer,
}
```


### Optional and Required fields

Most structure members are optional by default.

The [`@box` trait ](https://awslabs.github.io/smithy/1.0/spec/core/type-refinement-traits.html#box-trait) for a structure member means that it is not required to be present, and there is no default value. A boxed structure member would be emitted in Rust code with an `Option<>` wrapper.

The types boolean, byte, short, integer, long, float, and double types are not boxed unless they have an explicit `@box` annotation, in other words, without `@box` these types are required. All other types (string, list, map, structure, etc.) are implicitly boxed and without annotation would be optional.

The `@required` trait may be used on structure members to indicate that it must be present.


## Operations

Operations represent functions, and are declared with 0 or 1 input types (parameters)
and 0 or 1 output types (return values).

```text
/// Increment the value of the counter, returning its new value
operation Increment {
    input: I32,
    output: I32,
}
```

Operation input and output types can be any [supported data type](weld-smithy.md),
other than optional types.
An operation with no input declaration means the operation takes no parameters,
for example,
```text
operation GetTimeOfDay {
    output: Timestamp
}
```

An operation without output means the operation has no return value
(e.g., returns 'void'), for example,
```text
operation SetCounter {
    input: U64,
}
```

Input and output types cannot be optional. If you need to model a function
such as `fn lookup(key: String) -> Option<String>` or
`func resetCounter( value: number | null )` , you'll need to use a structure
with an optional field.


## Multiple parameters

We would like to model functions that take multiple parameters.
We can do that with Smithy by creating a wrapper structure for the multiple args.

For example, this method from  `wasmcloud:keyvalue`, defined in WIDL:
   ```text
   Set(key: string, value: string, expires: i32): SetResponse 
   ```
would not be written the same way in Smithy, since operations can only
have one input. Instead, the smithy declaration would be
   ```text
   operation Set {
     input: SetRequest,
     output: SetResponse,
   }
   structure SetRequest {
     @required
     key: String,
     @required
     value: String,
     @required
     expires: I32,
   }
   structure SetResponse {
     // ...
   }
   ```
WIDL's code generator constructs a request structure like SetRequest automatically,
so in fact the code generated from the above smithy model should be on-the-wire compatible with the widl-generated one. In both cases, the name of the wrapper struct is not transmitted, and the parameters are sent as a serialized map of key-value pairs.

In the future we would like the weld code generator to provide the same "syntactic sugar" that the widl generator does - function signatures with multiple args. A trait would be added to the structure (for examplel `@flattenInput`), and this would signal the code generator to generate the expanded function signature. This flattening ability is on the roadmap.

## Documentation

Documentation for shapes is indicated by preceding the shape declaration with one or more lines of 3-slash comments '/// Comment', and this documentation is emitted by code and documentation generators. This is equivalent internally to using the `@documentation` trait. In Smithy, documentation on consecutive lines is combined into a single block, and interpreted as __CommonMark markdown__. At the moment, html generated by the documentation generator does not perform markdown-to-html conversion, so documentation appears as it does in the source file.
