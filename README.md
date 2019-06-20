# WASCAP

A [WebAssembly Standard Capabilities Library](https://wascap.io) for Rust

If you just want the CLI that signs and examines capabilities claims, then you can just install it with cargo:
```
$ cargo install wascap
```

This library can be used for embedding, extracting, and validating capabilities claims
in WebAssembly modules. While there are some standard, well-known claims already defined,
you can add custom claims in your own namespaces if you like.
 
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
╔════════════════════════════════════════════════════════════════════════════╗
║                                WASCAP Module                               ║
╠═══════════════╦════════════════════════════════════════════════════════════╣
║ Account       ║   ACP6T7SH5R6JL3WV3LMNRS5V2SLB4LAMZR7CQPS6IAPYDW3OSBCTYM2J ║
╠═══════════════╬════════════════════════════════════════════════════════════╣
║ Module        ║   MABXCIBU2N2FORNPKRUINQEGES2V2NE4EVD6ZRE7DFIOIX6JE7SLR3U4 ║
╠═══════════════╬════════════════════════════════════════════════════════════╣
║ Expires       ║                                                      Never ║
╠═══════════════╬════════════════════════════════════════════════════════════╣
║ Can Be Used   ║                                                Immediately ║
╠═══════════════╩════════════════════════════════════════════════════════════╣
║                                Capabilities                                ║
╠════════════════════════════════════════════════════════════════════════════╣
║ K/V Store                                                                  ║
║ Messaging                                                                  ║
║ HTTP Client                                                                ║
║ HTTP Server                                                                ║
╠════════════════════════════════════════════════════════════════════════════╣
║                                    Tags                                    ║
╠════════════════════════════════════════════════════════════════════════════╣
║ None                                                                       ║
╚════════════════════════════════════════════════════════════════════════════╝
```