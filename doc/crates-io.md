# Rust crate structure, and crates.io

This document applies to "interface" crates that are built from `.smithy` models, and provides some information about how and when source files are updated.

In interface projects (in the source tree), there are one or more `.smithy` files, and a `codegen.toml`, which references the paths to the smithy files and to the output directories for the various languages.`codegen.toml` and `.smithy` files are language-agnostic, so they live one directory up from the `rust` folder, which contains `Cargo.toml` and `build.rs`.

If you want to publish an interface crate on crates.io, all the files necessary to generate the code for that library need to be in the published folder, and nothing in the src/ folder may change while the folder is being built on crates.io.

The usual method for dynamic source generation in rust crates is to create a `build.rs` script, which uses the environment variable OUT_DIR (set by cargo/rustc to a location deep inside the build/ folder) as the target for the generated file. Unfortunately IDEs and rust-analyzer aren't able to use that folder to find symbols, provide hints, go-to-definition, etc. (Confirmed in June 2021 with visual studio code and clion/intellij) For the best developer experience, we have set up all the example interface projects to generate code right into the src/ folder. To keep the build compatible with crates.io rules, build.rs is not published with the crate, so all src/* files are frozen until the next published crate version.

When building from the source tree, `rust/build.rs`, `codegen.toml`, and the `*.smithy` model files are present, so the generated interface library will automatically update and rebuild to keep up with any changes you make to the Smithy models. This seems to have the best combination of outcomes: stable interfaces for published releases, dynamic updates for local builds, and full IDE support.
