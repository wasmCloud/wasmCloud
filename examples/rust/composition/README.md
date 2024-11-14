# Composition Example

This is an example of **composing WebAssembly components**: combining multiple components into a single binary. The contents of this repo include:

* `/http-hello`: A directory containing a Rust-based HTTP "Hello World" component
* `/http-hello2`: A directory containing a modified Rust-based HTTP "Hello World" component using a custom interface
* `/pong`: A directory containing a Rust-based component exporting on a custom `pingpong` interface
* `compose.wac`: A file defining how components should be combined ("composed")
* `wadm.yaml`: A deployment manifest used with the final composed component

In this demo, we'll perform multiple compositions, using tools like **WAC** and **WASI Virt** to compose Wasm binaries that run in wasmCloud and Wasmtime, and ultimately arriving at a single composed component. We will also see how linking components at runtime can achieve the same effect as build-time composition in distributed environments.

# Installation

This demo is designed to demonstrate multiple ecosystem tools, including:

* **wasmCloud Shell (`wash`)**: The wasmCloud command-line interface (CLI) tool.
* **Wasmtime**: The standalone WebAssembly runtime.
* **WebAssembly Compositions (WAC)**: A CLI tool for declaratively composing components.
* **WASI Virt**: A CLI tool for virtualizing components within a composition.

Below are instructions for installing the required tooling for this example. Note that multiple tools require Cargo, and WASI Virt requires the [nightly release channel for Rust](https://github.com/bytecodealliance/WASI-Virt/blob/main/rust-toolchain.toml), so you may wish to install that up front:

```bash
rustup toolchain install nightly
```

## wasmCloud Shell (`wash`)

Follow the instructions for your OS on the [Installation page](https://wasmcloud.com/docs/installation). Since several of the following tools use [Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html), you may wish to use `wash` through Cargo as well.

```shell
cargo install --locked wash-cli
```

If you have [`cargo-binstall`][cargo-binstall] installed, you can install even faster:

```bash
cargo binstall wash-cli
```

[cargo-binstall]: https://crates.io/crates/cargo-binstall

## Wasmtime

On Linux or macOS, you can install Wasmtime locally with an install script:

```shell
curl https://wasmtime.dev/install.sh -sSf | bash
```

## WebAssembly Compositions (WAC)

WAC requires [`cargo`](https://doc.rust-lang.org/cargo/getting-started/installation.html). Once you've installed cargo:

```shell
cargo install wac-cli
```

## WASI Virt

WASI Virt requires the nightly release channel for [Rust](https://www.rust-lang.org/tools/install):

```bash
rustup toolchain install nightly
```

Install the `wasi-virt` command line tool with Rust's `cargo` package manager:

```bash
cargo +nightly install --git https://github.com/bytecodealliance/wasi-virt
```

## This example

These example files are included in the wasmCloud [monorepo](https://en.wikipedia.org/wiki/Monorepo). You can perform a "sparse" checkout to download only the relevant example directory (and associated documentation and metadata):

```bash
git clone --depth 1 --no-checkout https://github.com/wasmCloud/wasmCloud.git
cd wasmcloud
git sparse-checkout set ./examples/rust/composition
git checkout
cd examples/rust/composition
```

# Step 0: Run a component in wasmCloud and Wasmtime

First, we'll build our standard HTTP "hello world" example to set a baseline. From the project directory:

```shell
cd http-hello
wash build
```
We can use `wash inspect` to examine the WIT for this component:

```shell
wash inspect --wit build/http_hello_world_s.wasm
```
```wit
package root:component;

world root {
  import wasi:io/error@0.2.0;
  import wasi:io/streams@0.2.0;
  import wasi:http/types@0.2.0;
  import wasi:cli/environment@0.2.0;
  import wasi:cli/exit@0.2.0;
  import wasi:cli/stdin@0.2.0;
  import wasi:cli/stdout@0.2.0;
  import wasi:cli/stderr@0.2.0;
  import wasi:clocks/wall-clock@0.2.0;
  import wasi:filesystem/types@0.2.0;
  import wasi:filesystem/preopens@0.2.0;

  export wasi:http/incoming-handler@0.2.0;
}
```

Now we'll start a local wasmCloud host in detached mode, deploy the component, and invoke it with a `curl`:

```shell
wash up -d
wash app deploy wadm.yaml
curl localhost:8000
```
Once we see that everything is running as expected in wasmCloud, we can undeploy the app.

```shell
wash app undeploy http-hello-world
```
Now let's see how we can run the same `http-hello-world` component in the standalone Wasmtime runtime.

```shell
wasmtime serve -S cli=y build/http_hello_world.wasm
```
The component works exactly the same way. We can stop `wasmtime serve` with CTRL+C.

# Step 1: Virtualize a component

Now we'll move over to the `pong` directory. Here we have a component with a custom interface called `pong` that will return a string "ping" on its exported `pingpong` interface. Go ahead and build the component.
```shell
cd ../pong
wash build
```
We can use `wash inspect` to take a look at the WIT for this component:
```shell
wash inspect --wit ./build/pong_s.wasm
```
```wit
package root:component;

world root {
  import wasi:cli/environment@0.2.0;
  import wasi:cli/exit@0.2.0;
  import wasi:io/error@0.2.0;
  import wasi:io/streams@0.2.0;
  import wasi:cli/stdin@0.2.0;
  import wasi:cli/stdout@0.2.0;
  import wasi:cli/stderr@0.2.0;
  import wasi:clocks/wall-clock@0.2.0;
  import wasi:filesystem/types@0.2.0;
  import wasi:filesystem/preopens@0.2.0;
  import wasi:random/random@0.2.0;

  export example:pong/pingpong;
}
```
You can see the `pingpong` interface listed as an export and `wasi:cli/environment` as one of many imports.

Now it's time to perform our first composition in the form of a virtualization with WASI Virt. Using WASI Virt, we're going to compose `pong_s.wasm` into an encapsulating component with an environment variable `PONG` set to `demo`. The resulting component will be named `virt.wasm`.
```shell
wasi-virt build/pong_s.wasm --allow-random -e PONG=demo -o virt.wasm
```
Now let's view the WIT for our virtualized component:

```shell
wash inspect --wit virt.wasm
```
```wit
package root:component;

world root {
  import wasi:random/random@0.2.0;

  export example:pong/pingpong;
}
```
The virtualized component still exports `pingpong` but no longer requires `wasi:cli/environment`&mdash;that import (and all of the others except the new `wasi:random/random`, which we added via a WASI Virt argument) is satisfied by the encapsulating component. If we run this component on wasmCloud or with Wasmtime, the host will be able to satisfy the `random` import. But we still need another component to invoke this one via the exposed `pingpong` interface.

# Step 2: Linking at runtime

In the `http-hello2` directory, we have a modified `http-hello-world` component that imports the `pingpong` interface and calls for a string from `pong` to append to the hello world message. Let's navigate over to that directory and build the component.

```shell
cd ../http-hello2
wash build
```
The `wadm.yaml` deployment manifest in this directory will launch both the `pong` component from Step 1 and the new `http-hello-world` we just built. When we deploy with this manifest, wasmCloud will automatically **link the components at runtime**, so that `pong` can satisfy the import of `http-hello-world`.

```shell
wash app deploy wadm.yaml
```

When we invoke `http-hello-world` via `curl`, it invokes `pong`:

```shell
curl localhost:8000
Hello World! I got pong demo
```

For many use-cases, runtime linking is the most appropriate approach and we can stop here. To clean up our wasmCloud host environment:

```shell
wash app undeploy http-hello-world
wash app delete http-hello-world --delete-all
```

For the purposes of demonstration, let's see how we can compose these components into a single component at build-time.

# Step 3: Composing the component

For context, let's try running `wasmtime serve` *without* composing `pong` and `http-hello-world` together.

```shell
wasmtime serve -S cli=y build/http_hello_world.wasm
Error: component imports instance `example:pong/pingpong`, but a matching implementation was not found in the linker

Caused by:
    0: instance export `ping` has the wrong type
    1: function implementation is missing
```
As we might expect, it doesn't work&mdash;the hello world component doesn't have anything linked to fulfill the `pong` import. If we compose it with the `pong` component, however, we essentially wire those corresponding imports and exports together within the encapsulating component. Head back to the root of the project directory with `cd ..` and then we'll use WAC to perform the composition.

WAC is a tool for composing components at build-time. It has the ability to do complex compositions using the `.wac` file format (an example of which can be found below as an optional step). But for purposes of our example, we're going to use the `wac plug` command to perform the composition:

```shell
wac plug --plug pong/virt.wasm ./http-hello2/build/http_hello_world.wasm -o output.wasm
```

The "plug" in this example is the component exporting what the other component needs. So in our case, it is exporting `example:pong/pingpong`.

Now we have a new file: `output.wasm`. Taking a look at our composed component, we can see that we've ended up with a combination of both components:

```bash
wash inspect --wit output.wasm
```
```wit
package root:component;

world root {
  import wasi:random/random@0.2.0;
  import wasi:io/error@0.2.0;
  import wasi:io/streams@0.2.0;
  import wasi:http/types@0.2.0;
  import wasi:cli/environment@0.2.0;
  import wasi:cli/exit@0.2.0;
  import wasi:cli/stdin@0.2.0;
  import wasi:cli/stdout@0.2.0;
  import wasi:cli/stderr@0.2.0;
  import wasi:clocks/wall-clock@0.2.0;
  import wasi:filesystem/types@0.2.0;
  import wasi:filesystem/preopens@0.2.0;

  export wasi:http/incoming-handler@0.2.0;
}
```

Let's try running it with Wasmtime:

```shell
wasmtime serve -S cli=y output.wasm
```
```shell
curl localhost:8000
Hello World! I got pong demo
```
We can run the composed component in wasmCloud as well:

```shell
wash app deploy wadm.yaml
```
Congratulations! You've composed a component that runs anywhere supporting WASI P2.
For more information on linking components at runtime or at build via composition, including when you might want to use each approach, see [Linking Components](https://wasmcloud.com/docs/concepts/linking-components/) in the wasmCloud documentation.

## Advanced WAC

WAC can also work by encoding a composed component based on the instructions defined in a .wac file. The .wac file uses a superset of WIT also called WAC. This section contains an example that does the exact same thing as the `plug` command above, but explicitly defining it with a WAC file. In this example we have a `compose.wac` file. Here are the contents of that file:

```wit
package demo:composition;

let pong = new ping:pong { ... };
let hello = new hello:there { "example:pong/pingpong": pong.pingpong, ... };

export hello...;
```

This is a pretty simple composition, but the the design of WAC facilitates much more complex compositions of many components. You can learn more about WAC usage in the tool's [readme](https://github.com/bytecodealliance/wac/blob/main/README.md) and [language guide](https://github.com/bytecodealliance/wac/blob/main/LANGUAGE.md).

For this demo's purposes, we simply need to run `wac encode` while indicating the components we wish to compose with the `--dep` argument, naming our output file with `-o`, and specifying our `.wac` instructions:

```shell
wac encode --dep ping:pong=./pong/virt.wasm --dep hello:there=./http-hello2/build/http_hello_world_s.wasm -o output.wasm compose.wac
```

You can then run the `output.wasm` in the same way as above!
