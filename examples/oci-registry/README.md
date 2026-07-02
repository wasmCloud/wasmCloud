# OCI Registry

A minimal [OCI Distribution Spec][oci-dist] (v2) registry implemented as a single
**wasip3** WebAssembly component. It exports `wasi:http/handler@0.3.0` to serve
the registry API and stores every blob, manifest, and tag through the async,
native-stream [`wasmcloud:blobstore@0.1.0`][wasmcloud-blobstore] interface, so it
runs against any blobstore backend (in-memory, filesystem, NATS, …) without code
changes.

It is spec compliant and can be used as a drop-in target for real clients such as
[`oras`][oras] and `docker`/`podman`: blob CRUD, manifest CRUD, resumable
uploads, tag listing, and the referrers API.

[oci-dist]: https://github.com/opencontainers/distribution-spec/blob/main/spec.md
[oras]: https://oras.land
[wasmcloud-blobstore]: https://github.com/wasmCloud/wasmCloud/pull/5297

## Prerequisites

- `cargo` (Rust 2024 edition)
- A `wash` built from the [`async-backends`][async-backends] line — it provides
  the async `wasmcloud:blobstore` host plugin (enabled with the
  `wasm_component_model_implements` feature) and a `wash` CLI that can build and
  run wasip3 components. A released `wash` (≤ 2.x) does not yet support this.
- Optional, for the walkthrough: [`oras`](https://oras.land/docs/installation)

[async-backends]: https://github.com/wasmCloud/wasmCloud/pull/5297

## Running with wash

```shell
wash dev
```

This builds the component and serves it on [http://localhost:8000](http://localhost:8000),
wiring up an HTTP server and the async blobstore host plugin.

The `dev.host_interfaces` entry in `.wash/config.yaml` routes the `(implements ..)`
blobstore label `store` to the **filesystem** backend rooted at `tmp/blobstore`,
so registry contents persist across `wash dev` restarts:

```yaml
dev:
  host_interfaces:
    - namespace: wasmcloud
      package: blobstore
      interfaces: [blobstore]
      version: "0.1.0"
      name: store              # matches the `import store:` label in wit/world.wit
      config:
        backend: filesystem    # omit host_interfaces entirely to use in-memory
        root: tmp/blobstore
```

## Building

```shell
wash build
```

`wash build` runs the `cargo build --target wasm32-wasip2` from `.wash/config.yaml`;
the linker componentizes the result into a wasip3 component (imports
`wasmcloud:blobstore/*@0.1.0`, exports `wasi:http/handler@0.3.0`).

## Required Capabilities

1. `wasi:http` to receive registry requests (wasip3 `handler@0.3.0`)
2. `wasmcloud:blobstore` to persist blobs, manifests, and tags
3. `wasi:random` to mint upload-session identifiers

## Supported endpoints

| Operation         | Method   | Path                                            |
| ----------------- | -------- | ----------------------------------------------- |
| API version check | `GET`    | `/v2/`                                           |
| Initiate upload   | `POST`   | `/v2/<name>/blobs/uploads/`                       |
| Cross-repo mount  | `POST`   | `/v2/<name>/blobs/uploads/?mount=<digest>&from=<repo>` |
| Upload a chunk    | `PATCH`  | `/v2/<name>/blobs/uploads/<session>`             |
| Complete upload   | `PUT`    | `/v2/<name>/blobs/uploads/<session>?digest=<d>`  |
| Pull a blob       | `GET`    | `/v2/<name>/blobs/<digest>`                       |
| Check a blob      | `HEAD`   | `/v2/<name>/blobs/<digest>`                       |
| Delete a blob     | `DELETE` | `/v2/<name>/blobs/<digest>`                       |
| Push a manifest   | `PUT`    | `/v2/<name>/manifests/<reference>`               |
| Pull a manifest   | `GET`    | `/v2/<name>/manifests/<reference>`               |
| Check a manifest  | `HEAD`   | `/v2/<name>/manifests/<reference>`               |
| Delete a manifest | `DELETE` | `/v2/<name>/manifests/<reference>`               |
| List tags         | `GET`    | `/v2/<name>/tags/list[?n=&last=]`                 |
| List referrers    | `GET`    | `/v2/<name>/referrers/<digest>[?artifactType=]`   |

`<name>` may contain slashes (e.g. `library/nginx`). A `<reference>` is either a
tag or a `sha256:<hex>` digest. Both monolithic (single `PUT`) and chunked
(`PATCH` then `PUT`) blob uploads are supported, and uploaded content is verified
against the client-supplied digest before it is committed.

A few protocol details that the conformance suite exercises:

- **Chunked uploads** honor `Content-Range`: a chunk whose start offset doesn't
  match the current session length (out-of-order or replayed) is rejected with
  `416 Range Not Satisfiable`.
- **Tag listing** supports `n` (page size) and `last` (resume after a tag), and
  emits a `Link: ...; rel="next"` header when results are truncated.
- **Referrers**: pushing a manifest with a `subject` field indexes it (and sets
  the `OCI-Subject` response header); `GET .../referrers/<digest>` returns an OCI
  image index of the referring descriptors, filterable by `artifactType`.
- **Cross-repository mount**: `POST .../uploads/?mount=<digest>&from=<repo>`
  copies an existing blob into the target repository without re-uploading,
  returning `201` (or `202` with a normal upload session if the source blob
  isn't found). Automatic content discovery (`?mount=` without `from`) is not
  implemented — that variant returns a `202` upload session.
- Deleting a manifest by digest removes the content; deleting by tag only removes
  that tag (other tags pointing at the same digest are left intact).

## Try it with `oras`

```console
# Push an artifact
$ echo 'hello oci world' > hello.txt
$ oras push --plain-http 127.0.0.1:8000/myrepo/artifact:v1 hello.txt:text/plain
...
Pushed [registry] 127.0.0.1:8000/myrepo/artifact:v1
Digest: sha256:...

# List tags
$ oras repo tags --plain-http 127.0.0.1:8000/myrepo/artifact
v1

# Pull it back into a clean directory
$ mkdir /tmp/pulled && cd /tmp/pulled
$ oras pull --plain-http 127.0.0.1:8000/myrepo/artifact:v1
$ cat hello.txt
hello oci world
```

## Serving Wasm components

Because it's a spec-compliant registry, this component can host `.wasm`
components as OCI artifacts — i.e. act as a component registry for the wasmCloud
toolchain. Push a built component with `wash oci push`, then pull it back with
any OCI client:

```console
# Push a built component to this registry (--insecure = plain HTTP, no auth)
$ wash oci push --insecure \
    localhost:8000/library/oci-registry:0.1.0 \
    target/wasm32-wasip2/release/oci_registry.wasm
OCI command executed successfully.

# Pull it back — wash, wkg, and oras all consume it, byte-for-byte identical
$ wash oci pull --insecure localhost:8000/library/oci-registry:0.1.0   # -> /tmp/component.wasm
$ wkg  oci pull localhost:8000/library/oci-registry:0.1.0 --insecure localhost:8000 -o out.wasm
$ oras pull --plain-http localhost:8000/library/oci-registry:0.1.0
```

`wash oci push` stores a canonical Wasm OCI artifact — verify what the registry
is serving:

```console
$ curl -s localhost:8000/v2/library/oci-registry/manifests/0.1.0 | jq '{config: .config.mediaType, layer: .layers[0].mediaType, size: .layers[0].size}'
{
  "config": "application/vnd.wasm.config.v0+json",
  "layer": "application/wasm",
  "size": 373980
}

$ curl -s localhost:8000/v2/library/oci-registry/tags/list
{"name":"library/oci-registry","tags":["0.1.0"]}
```

The layer digest equals the `sha256` of the original `.wasm`, so the component
round-trips through the registry unchanged.

> To have a wasmCloud host *run* a component straight from this registry,
> reference `localhost:8000/library/...:<tag>` as the `image` in a wadm manifest.
> The host must be configured to allow the insecure (plain-HTTP, no-auth)
> registry, otherwise it refuses the pull.

## Try it with `curl`

```console
# API version check
$ curl -i http://127.0.0.1:8000/v2/
HTTP/1.1 200 OK
docker-distribution-api-version: registry/2.0

# Monolithic blob upload: initiate, then PUT with the digest
$ BLOB='example blob'
$ DIGEST="sha256:$(printf '%s' "$BLOB" | shasum -a 256 | cut -d' ' -f1)"
$ LOC=$(curl -s -D - -o /dev/null -X POST \
    http://127.0.0.1:8000/v2/demo/blobs/uploads/ \
    | tr -d '\r' | awk -F': ' 'tolower($1)=="location"{print $2}')
$ curl -i -X PUT "http://127.0.0.1:8000${LOC}?digest=${DIGEST}" --data-binary "$BLOB"
HTTP/1.1 201 Created
docker-content-digest: sha256:...

# Pull the blob back
$ curl "http://127.0.0.1:8000/v2/demo/blobs/${DIGEST}"
example blob
```

## How it works

Every object lives in one blobstore container (`oci-registry`), keyed by
repository name:

| Kind             | Object key                              |
| ---------------- | --------------------------------------- |
| Blob             | `<name>/blobs/sha256_<hex>`             |
| Manifest content | `<name>/manifests/sha256_<hex>`         |
| Manifest type    | `<name>/manifests/sha256_<hex>.mediatype` |
| Tag → digest     | `<name>/tags/<tag>`                      |
| Upload session   | `<name>/uploads/<session-id>`           |
| Referrer         | `<name>/referrers/<subject>/<manifest>` |

Digests contain a `:` separator that is not portable across all blobstore
backends, so it is replaced with `_` in object keys. Blobs are content-addressed
and scoped per repository; tags are small pointer objects holding the digest they
resolve to, and each referrer is a stored descriptor keyed by its subject.

## Conformance

This example passes the official
[OCI distribution-spec conformance suite][conformance] across all four
categories (pull, push, content discovery, content management). The suite is a Go
test binary you point at any running registry.

**Prerequisite:** [Go](https://go.dev/dl/) 1.21+.

1. Start this registry in one terminal:

   ```shell
   wash dev    # serves the registry on http://localhost:8000
   ```

2. In another terminal, build the conformance binary from a checkout of the
   distribution-spec repo (the `v1.1.0` tag uses the `OCI_*` env vars below;
   newer revisions renamed them to `OCI_REGISTRY`/`OCI_REPO1`/…):

   ```shell
   git clone --depth 1 --branch v1.1.0 https://github.com/opencontainers/distribution-spec
   cd distribution-spec/conformance
   go test -c -o conformance.test
   ```

3. Run it against the registry. The four `OCI_TEST_*` toggles enable each
   category; use a **fresh** `OCI_NAMESPACE` per run so leftover tags from a
   previous run don't skew the tag-pagination checks:

   ```shell
   OCI_ROOT_URL=http://localhost:8000 \
   OCI_NAMESPACE=conformance/oci-registry \
   OCI_TEST_PULL=1 \
   OCI_TEST_PUSH=1 \
   OCI_TEST_CONTENT_DISCOVERY=1 \
   OCI_TEST_CONTENT_MANAGEMENT=1 \
   OCI_CROSSMOUNT_NAMESPACE=conformance/oci-registry-mount \
   OCI_REPORT_DIR=report \
   ./conformance.test
   ```

   Expected result (against the async `wasmcloud:blobstore` backend):

   ```text
   Ran 75 of 80 Specs
   SUCCESS! -- 75 Passed | 0 Failed | 0 Pending | 5 Skipped
   ```

   `OCI_REPORT_DIR` also writes a human-readable `report/report.html`. Setting
   `OCI_CROSSMOUNT_NAMESPACE` exercises cross-repository blob mount (which this
   registry implements). The remaining skips are inapplicable spec branches: the
   suite's alternate "pre-populated registry" setup (two specs, the unused half of
   an either/or with the push-based setup that runs), the mount branch not taken
   (`202` "nonexistent" — we return `201`), and automatic content discovery
   (`?mount=` without `from`, an opt-in sub-feature this registry doesn't provide).

[conformance]: https://github.com/opencontainers/distribution-spec/tree/main/conformance

## Limitations

- There is no authentication.
- **Streaming** takes advantage of the native `wasmcloud:blobstore` `stream<u8>`
  bodies where it can: blob **pulls** (`GET`) pipe the blobstore `get-data`
  stream straight into the HTTP response, and **monolithic** blob pushes stream
  the request body into the blobstore while hashing it, so neither holds the
  layer in memory. Two paths still buffer: **manifests** (small JSON that must be
  hashed and parsed for `subject`/referrers) and **chunked `PATCH`** uploads —
  `wasmcloud:blobstore` has no append, so each chunk rewrites the whole session
  object and the finalizing `PUT` streams that buffered prefix first.
- Cross-repository mount requires an explicit `from` repository; automatic
  content discovery (`?mount=` without `from`) is not implemented and returns a
  normal upload session.
