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
Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands. Once the application reports as **Deployed** in the application list, you can use `wash call` to call the exported function `invoke`.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash call ferris_says-ferris_says wasmcloud:example-ferris-says/invoke.say
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

## BONUS: Using the "system" clock in a WebAssembly context

When you use the `say()` export with this component, it uses the system clock, provided by the [Rust standard library (`SystemTime`)][rust-systemtime] and made available via [WASI clocks interface][wasi-clocks]. 

Time, and computerized clocks are complicated, and "base" WebAssembly has *no concept* of a clock, which is where the WASI clocks interface steps in. That said, it's not that simple -- [`wasmtime`][wasmtime] and other implementers of the WASI spec must *choose* what implementation of a clock to give you. 

While somewhat unexpected, giving an accurate clock *can be unsafe*. Giving untrusted programs access to a very accurate clock on a system can make [timing attacks possible](https://en.wikipedia.org/wiki/Timing_attack) or make for [easier fingerprinting](https://github.com/bytecodealliance/wasmtime/issues/2125), so some may choose to insert some jitter into the clock that is made available to WebAssembly binaries at runtime.

wasmCloud chooses to expose the [`wasmtime` generated bindings for the system clock](https://docs.rs/wasmtime-wasi/latest/wasmtime_wasi/bindings/sync/clocks/wall_clock/trait.Host.html), which *may* change in the future, but it's important to keep in mind that these discrepancies are present, and answers you get from the SystemTime *may* be slightly inconsistent (in a bounded way) with "real time".

[wasi-clocks]: https://github.com/WebAssembly/wasi-clocks
[rust-systemtime]: https://doc.rust-lang.org/std/time/struct.SystemTime.html
[wasmtime]: https://wasmtime.dev
