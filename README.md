# Weld - using Smithy models with wasmCloud

Weld is a tool framework for using [Smithy IDL](https://awslabs.github.io/smithy/index.html) for [wasmCloud](https://wasmcloud.dev).

This repository contains

- The `weld` cli ([installation](doc/prerequisites.md/#weld)) ([source](./bin)) containing a code generator, html documentation generator, a model linter, and a model validator. Note that if you are looking for _ready-to-run_ code generation of wasmCloud actors, providers, and interfaces, you should be using [wash](https://github.com/wasmCloud/wash) rather than weld.
- [wasmbus-rpc](./rpc-rs) library used by actors and capability providers to send and receive messages. This library also contains the generated interface library for the wasmcloud core models.

## Documentation

- Getting started
  - Install the [weld](doc/prerequisites.md#weld) cli tool and [prerequisites](doc/prerequisites.md)
  - Creating a new **actor**, **interface**, or **provider** should be done through [wash](https://github.com/wasmCloud/wash), a CLI that relies on various **weld** components.
    
- Install the Visual Studio plugin for Smithy syntax highlighting (in extensions marketplace or from [github](https://github.com/awslabs/smithy-vscode))
  


- Guides
  - [Weld-Smithy guide](doc/weld-smithy.md) - examples of how weld uses smithy models, with examples.
  - [codegen.toml guide](doc/codegen-toml.md): instructions and examples for editing a project `codegen.toml`.
  - [Code generation](doc/code-generation.md) - more info about how code and documentation generation works, and how to customize it or write a new code generator.
  - [Tips for building and debugging models](doc/getting-started.md#model-builds-and-debugging)
  - [Publishing](doc/crates-io.md) rust interface libraries to crates.io
  

  - Tips and suggestions
    - [simplify single-member structures](doc/tips/single-member-structures.md)
  
  
## Rust-atelier

Weld makes heavy use of [rust-atelier](https://github.com/johnstonskj/rust-atelier), a Rust implementation of [AWS Smithy](https://awslabs.github.io/smithy/index.html).


## Smithy References and tools

- [Smithy home page](https://awslabs.github.io/smithy/index.html)
- [IDL spec v1.0](https://awslabs.github.io/smithy/1.0/spec/core/idl.html)
- [ABNF grammar](https://awslabs.github.io/smithy/1.0/spec/core/idl.html#smithy-idl-abnf) (if you want to write your own parser)
- [JSON AST](https://awslabs.github.io/smithy/1.0/spec/core/json-ast.html) - the handlebars-based code generators use this
  
- [Specifications](https://awslabs.github.io/smithy/1.0/spec/index.html)

- [Visual Studio plugin](https://github.com/awslabs/smithy-vscode) (in the extension marketplace)

- [Rust-atelier](https://github.com/johnstonskj/rust-atelier)
  
- SDKs (code generators and tools below are implemented in Java)
  - Go [github](https://github.com/aws/smithy-go)
  - Java sdk [github](https://github.com/awslabs) ( [javadoc](https://awslabs.github.io/smithy/javadoc/1.8.0/)  )
  - Rust [github](https://github.com/awslabs/smithy-rs) (Alpha status)
  - TypeScript [github](https://github.com/awslabs/smithy-typescript)


# Status

This is a work in progress - offers of help are much appreciated! Current status:

|Done? | |
| :--- | :--- |
| [x] | Getting started documentation|
| [x] | Intermediate documentation|
| [x] | model lint |
| [x] | model validate |
| [x] | HTML documentation generation |
| | __Examples__ |
| [x] | Interface
| [x] | Actor
| [ ] | Provider (OTP)
| | __Rust code-gen/project-gen__ |
| [x] | Rust Actor
| [x] | Rust Interface
| [ ] | Rust Provider (OTP) |


## Future

- Assemblyscript codegen
  - Interface
  - Actor
- Go code-gen 
  - Interface
  - Provider (OTP)
- Grain code-gen
  - Interface
  - Actor
