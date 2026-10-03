# Linked resource destruction regression

Based on wash v2.10.1 (fd2bbc0). The dynamic linker previously registered a
no-op destructor for ResourceAny proxy resources. Dropping the caller's handle
therefore reclaimed neither the provider's guest object nor its ResourceTable
entry.

The fix removes the proxy entry and awaits resource_drop_async under a
StoreActiveCtxGuard for the exporting component. This both forwards destruction
and ensures any host imports made by that destructor use the provider's context.
The guard restores the previous context on return/error.

A related return-path bug wrapped already-wrapped ResourceAny handles again.
lift now preserves these handles by identity, matching the existing argument
lowering behaviour.

## Targeted reproduction

Build the three resource fixtures plus the fixture included by the common test
helpers. Use an existing wash binary for componentisation:

```sh
for f in res-producer-p3 res-sink-p3 res-caller-p3 postgres-stream-p3; do
  wash -C crates/wash-runtime/tests/fixtures/$f build
  mkdir -p crates/wash-runtime/tests/wasm
  cp crates/wash-runtime/tests/fixtures/target/wasm32-wasip1/release/${f//-/_}.wasm \
    crates/wash-runtime/tests/wasm/
done

features=washlet,oci,wasi-config,wasi-logging,wasi-blobstore,wasi-keyvalue,wasmcloud-postgres,wasi-otel,wasmcloud-nats

CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p wash-runtime \
  --no-default-features --features "$features" --test integration_p3_resources

CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p wash-runtime \
  --no-default-features --features "$features" --lib engine::value::tests
```

These feature selections match a wash build without default WebGPU/implements
features. They do not claim coverage of every optional runtime feature.

## Observed results

- Restoring only the old no-op destructor makes the integration test fail:
  **385 created, zero destroyed**, versus 385 expected destructions.
- With the fix: the initial owned transfer plus eight warm requests create and
  destroy **3,073 resources exactly once**.
- Counters persist across requests; this is not a store-teardown test.
- Borrowed method calls do not destroy the resource.
- Owned handles survive a third-component round trip and can still be used.
- Destructors see the provider's environment through direct WASI imports.
  The sink and caller subsequently see their own environments.
- All six engine::value tests pass, including the new identity regression.

A separate SQLx/WIT database experiment also passed using the patched wash
binary with its explicit-discard workaround disabled: abandoned PostgreSQL
transactions closed their actual backend sessions and released pool permits.
The installed unpatched wash failed that same test.

This is targeted lifecycle coverage, not an RSS soak test or a claim that every
runtime resource bridge has been audited. The separate cross-store component
plugin proxy bridge is not changed by this patch.
