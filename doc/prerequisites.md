# Prerequisites

You'll need the following packages for using weld and wasmcloud-related tools.
- [jq](#jq)
- [make](#make)
- [Rust SDK](#rust-sdk)
- [wasmcloud and wash](#wasmcloud-and-wash)
- [weld](#weld)

## jq

`jq` is a utility to extract information from json files

- debian/ubuntu: `sudo apt-get install jq`
- arch: `sudo pacman -Sy jq`
- mac: `brew install jq`


## make

Install GNU make

- debian/ubuntu: `sudo apt-get install make`
- arch: `sudo pacman -Sy make`  
- mac: `brew install make` (or XTools)


## Rust SDK

You'll need the Rust SDK for some of the wasmcloud tools, even if you don't plan to create actors or capability providers in Rust.

The Rust Language page has [installation instructions](https://www.rust-lang.org/tools/install0)

The examples should all build with either the `stable` or `nightly` channel.


## wasmcloud and wash

Once you have the rust tools installed, it's easy to install these:

```sh
cargo install wasmcloud
cargo install wash
```

The executables are installed into `$HOME/.cargo/bin` - make sure that's in your PATH.


## weld

The weld tool validates smithy models, is used to create projects and process Smithy models

```sh
cargo install wasmcloud-weld-bin
```

The `weld` binary will be installed in `$HOME/.cargo/bin`

Sources are in [github](https://github.com/wasmcloud/weld)



