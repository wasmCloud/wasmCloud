# Weld-Smithy Guide

Weld's use of Smithy closely follows the [Smithy IDL specification](https://awslabs.github.io/smithy/1.0/spec/core/idl.html). This document is intended to be a quick reference to the major features of Smithy, and includes some conventions on the way we use Smithy with `weld`.

__Contents__

- [Data types](#data-types)
- [Structures](#structures)  
- [Operations](#operations)
- [Documentation](#documentation)


## Models

We author models in Smithy IDL files, with a `.smithy` extension. An IDL file contains zero or one namespaces, 

A "Semantic Model", which can be serialized as a json file, can be built from multiple IDL and/or json (semantic model) files, and can contain multiple namespaces. To fully `weld lint` or `weld validate` a model, you will probably need to specify paths of all dependencies in all namespaces used by the model. `weld` commands accept multiple files, directories, or urls on the command line. Directories are scanned recursively to find `.smithy` or `.json` files to load.

The weld tool uses a configuration file, [codegen.toml](./codegen-toml.md), usually located at the project root folder.


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

The following Smithy shapes are __partially supported__: `set`, `Timestamp`

The following Smithy shapes are __not supported__ (yet): `BigInteger`, `BigDecimal`, `union`, `document`.

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

We would like to model functions that take multiple parameters,
such as `fn set(key: string, value: string)`, but Smithy only supports
a single `input` for any operation. Today, these parameters can be
wrapped up into either a map or a structure, and the map or structure
used as the input type. In WIDL, wrapping of multiple values
into a single input parameter was done
by the code generator, but there was no explicit indicator
in the interface IDL that this was occurring.

In Smithy, we want to make this explicit.

In the current weld version, the Smithy model author
must explicitly declare the wrapper structure, and the code generator
will generate function signatures with a single input parameter.

In a future release, a trait will be available to declare that an operation's
input structure should be "flattened" into multiple input parameters.
The input parameters will be in the same order as they appear
in the structure declaration.
If structure field members are declared as optional (e.g., do not have
a `@required` trait), they would appear as optional values in
the target language, such as `Option<Type>` or `Type | null`


```text
/// Add a key/value pair to the dictionary
@flattenInput
operation Set { 
    input: KeyValue, 
}

/// Key-value pair for Set
structure KeyValue {
    key: String,
    value: String,
}
```

Note that adding the `@flattenInput` trait to any operation would be a breaking
change to users of your api, since it will change the generated function signature.
If you plan to use this trait in the future, it may be useful to plan
to use a different namespace to use with param flattening to avoid incompatibilities.


## Documentation

Documentation for shapes is indicated by preceding the shape declaration with one or more lines of 3-slash comments '/// Comment', and this documentation is emitted by code and documentation generators. This is equivalent internally to using the `@documentation` trait. In Smithy, documentation on consecutive lines is combined into a single block, and interpreted as __CommonMark markdown__. At the moment, html generated by the documentation generator does not perform markdown-to-html conversion, so documentation appears as it does in the source file.
