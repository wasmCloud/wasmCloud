# Weld - using Smithy models with wasmcloud


Weld is based on the [Smithy IDL](https://awslabs.github.io/smithy/index.html) specification by Amazon.

The main types in a Smithy model are [shapes](https://awslabs.github.io/smithy/1.0/spec/core/model.html#shapes) (types), [services](https://awslabs.github.io/smithy/1.0/spec/core/model.html#service) (similar to interfaces), and [operations](https://awslabs.github.io/smithy/1.0/spec/core/model.html#operation) (methods). Objects can be annotated with [traits](https://awslabs.github.io/smithy/1.0/spec/core/model.html#traits), a flexible mechanism for declaring requirements, constraints, behaviors, or documentation.

The `weld` cli includes a code generator, documentation generator, a model linter, and a model validator, and can be installed with `cargo install wasmcloud-weld-bin` (requires [`cargo`](https://doc.rust-lang.org/cargo/getting-started/installation.html))


## Documentation


- Getting started
  - Install the [weld](doc/prerequisites.md#weld) cli tool and [prerequisites](doc/prerequisites.md)
  - Look at the [examples](./examples/) folder for common interfaces, actors, and capability providers.
  - Create a rust [actor](doc/getting_started.md#creating-an-actor) project
  - Create a rust [interface](doc/getting_started.md#creating-an-interface-project) project

- Install the Visual Studio plugin for Smithy syntax highlighting (in extensions marketplace or from [github](https://github.com/awslabs/smithy-vscode)


- Guides
  - Read the [Weld-Smithy guide](doc/weld-smithy.md) for examples of how weld uses smithy models, with examples.
  - Read [codegen.toml guide](doc/codegen-toml.md) for instructions and examples for editing a project `codegen.toml`.


  - Tips and suggestions
    - [simplify single-member structures](doc/tips/single-member-structures.md)
  

## Smithy References and tools

- [Smithy home page](https://awslabs.github.io/smithy/index.html)
- [IDL spec v1.0](https://awslabs.github.io/smithy/1.0/spec/core/idl.html)
- [ABNF grammar](https://awslabs.github.io/smithy/1.0/spec/core/idl.html#smithy-idl-abnf) (if you want to write your own parser)
- [JSON AST](https://awslabs.github.io/smithy/1.0/spec/core/json-ast.html) - the handlebars-based code generators use this
  
- [Specifications](https://awslabs.github.io/smithy/1.0/spec/index.html)

- Tools
  - [Visual Studio plugin](https://github.com/awslabs/smithy-vscode) (just search the extension marketplace for easy installation)
  - [Rust Atelier](https://github.com/johnstonskj/rust-atelier). Contains cli (`cargo-atelier`) that performs lint, validation, and model converion (various formats). Not necessary if you are using `weld`.
    
- SDKs (code generators and tools below are implemented in Java)
  - [Java SDK - main repo](https://github.com/awslabs) ( [javadoc](https://awslabs.github.io/smithy/javadoc/1.8.0/)  )
  - [Go](https://github.com/aws/smithy-go)
  - [Typescript](https://github.com/awslabs/smithy-typescript)
  - [Rust](https://github.com/awslabs/smithy-rs) (Alpha)


# Status

This is a work in progress - offers of help are much appreciated! Current status:

|Done? | |
| :--- | :--- |
| [ ] | Getting started documentation (50%) |
| [x] | lint |
| [x] | validate |
| [x] | HTML documentation generation |
| [ ] | code generation Rust: [x] Actor, [x] Interface library, [25%] Provider (OTP)|
| [ ] | code generation AssemblyScript: [ ] Actor |
| [ ] | code generation Go: [ ] Provider (OTP) |

