# Ferris Says

This folder contains a simple WebAssembly component written in [Rust][rust], which responds with a version of [cowsay][wiki-cowsay] tailored to Rustaceans (users of Rust).

[rust]: https://rust-lang.org
[wiki-cowsay]: https://en.wikipedia.org/wiki/Cowsay

## Prerequisites

- `cargo` 1.80
- [`wash`](https://wasmcloud.com/docs/installation) 0.29.0

## Building

```bash
wash build
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands. Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash call ferris-says wasmcloud:example-ferris-says/invoke.say
```

You should see output that looks like the following:

```
 _______________________________
< Hello fellow wasmCloud users! >
 -------------------------------
        \
         \
            _~^~^~_
        \) /  o o  \ (/
          '_   -   _'
          / '-----' \

```

## Adding Capabilities

Want to go beyond this simple demo?

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
