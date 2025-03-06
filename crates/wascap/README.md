![crates.io](https://img.shields.io/crates/v/wascap.svg)&nbsp;
![Rust](https://github.com/wasmcloud/wascap/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wascap.svg)

# ℹ️ Compatibility Information

The hashes computed with v`0.10.1` and later of wascap are not compatible with the hashes signed by prior versions. As a result, modules signed with older versions of wascap will _not_ have their module hashes validated (they'll be ignored). Once the module has been signed with `0.10.1` or greater, it will go back to having its module hash verified.

# wasmCloud Capabilities

In the [wasmCloud](https://wasmcloud.dev) host runtime, each component securely declares the set of capabilities it requires. This library is used to embed, extract, and validate JSON Web Tokens (JWT) containing these capability attestations, as well as the hash of the `wasm` file and a provable issuer for verifying module provenance.

If you want to use the CLI that lets you sign and examine module claims, then you can install the [wash](https://github.com/wasmCloud/wasmCloud/tree/main/crates/wash) CLI and use the `wash claims` set of commands. _Note that earlier versions of `wascap` came with a CLI. This is no longer available and has been supersede by the `wash` CLI._

While there are some standard, well-known claims already defined in the library (such as `wasmcloud:messaging` and `wasmcloud:keyvalue`), you can add custom claims in your own namespaces.

The following example illustrates embedding a new set of claims into a WebAssembly module, then extracting, validating, and examining those claims:

```rust
use wascap::prelude::*;

let unsigned = read_unsigned_wasm(); // Read a Wasm file into a byte vector
let issuer = KeyPair::new_account(); // Create an Ed25519 key pair to sign the module
let module = KeyPair::new_module(); // Create a key pair for the module itself

// Grant the module some basic capabilities, with no date limits
let claims = ClaimsBuilder::new()
    .with_capability(caps::MESSAGING)
    .with_capability(caps::KEY_VALUE)
    .issuer(&issuer.public_key())
    .subject(&module.public_key())
    .build();

// Sign the JWT and embed it into the WebAssembly module, returning the signed bytes
let embedded = wasm::embed_claims(&unsigned, &claims, &issuer)?;

// Extract a signed JWT from a WebAssembly module's bytes (performs a check on
// the signed module hash)
let extracted = wasm::extract_claims(&embedded)?.unwrap();

// Validate dates, signature, JWT structure, etc.
let v = validate_token(&extracted.jwt)?;

assert_eq!(v.expired, false);
assert_eq!(v.cannot_use_yet, false);
assert_eq!(v.expires_human, "never");
assert_eq!(v.not_before_human, "immediately");
assert_eq!(extracted.claims.issuer, issuer.public_key());
```

The `Ed25519` key functionality is provided by the [nkeys](https://docs.rs/nkeys) crate.

The `wash` CLI allows you to examine and sign WebAssembly files from a terminal prompt:

```terminal
 $ wash claims inspect echo_s.wasm

                               Echo - Module
  Account       ACM7TOENKEIO6Q6J66FX53AKXRHH6TH2WZ6K6SZQULNSWLDUIQHSRRSS
  Module        MC2N2ERC4J7GGMVEMH2TYY73YXYZT2BJK2VGFNYWV26K3KOSVALIFUIB
  Expires                                                          never
  Can Be Used                                                immediately
  Version                                                       None (0)
  Call Alias                                                   (Not set)
                               Capabilities
  HTTP Server
                                   Tags
  None
```
