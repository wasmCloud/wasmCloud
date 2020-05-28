![crates.io](https://img.shields.io/crates/v/wascap.svg)&nbsp;
![travis](https://travis-ci.org/wascc/wascap.svg?branch=master)&nbsp;
![license](https://img.shields.io/crates/l/wascap.svg)

# WASCAP

In the [waSCC](https://wascc.dev) runtime, each actor securely declares the set of capabilities it requires. This library and its associated binary are used to embed, extract, and validate JSON Web Tokens (JWT) containing these capability attestations, as well as the hash of the `wasm` file and a provable issuer for verifying module provenance.

If you just want the CLI that signs and examines capabilities claims, then you can install it with cargo:
```
$ cargo install wascap --features "cli"
```

While there are some standard, well-known claims already defined in the library (such as `wascc:messaging` and `wascc:keyvalue`), you can add custom claims in your own namespaces.

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

The `wascap` CLI allows you to examine and sign WebAssembly files from a terminal prompt:

```terminal
 $ wascap caps examples/signed_loop.wasm
 ╔════════════════════════════════════════════════════════════════════════╗
 ║                          Secure Actor - Module                         ║
 ╠═════════════╦══════════════════════════════════════════════════════════╣
 ║ Account     ║ ACCHS57D3P2VEON5MQCJM4YA34GYBDFZR3IBG5EQNUONIHBO5X4NIURC ║
 ╠═════════════╬══════════════════════════════════════════════════════════╣
 ║ Module      ║ MBQ2RC3BARXFWTBFW5UJ6J3QSAVYJ7D64Z5LRCPR3UI44F65Q3OMNGYM ║
 ╠═════════════╬══════════════════════════════════════════════════════════╣
 ║ Expires     ║                                                    never ║
 ╠═════════════╬══════════════════════════════════════════════════════════╣
 ║ Can Be Used ║                                              immediately ║
 ╠═════════════╬══════════════════════════════════════════════════════════╣
 ║ Version     ║                                                1.0.0 (0) ║
 ╠═════════════╩══════════════════════════════════════════════════════════╣
 ║                              Capabilities                              ║
 ╠════════════════════════════════════════════════════════════════════════╣
 ║ K/V Store                                                              ║
 ║ Messaging                                                              ║
 ║ HTTP Client                                                            ║
 ║ HTTP Server                                                            ║
 ╠════════════════════════════════════════════════════════════════════════╣
 ║                                  Tags                                  ║
 ╠════════════════════════════════════════════════════════════════════════╣
 ║ None                                                                   ║
 ╚════════════════════════════════════════════════════════════════════════╝
