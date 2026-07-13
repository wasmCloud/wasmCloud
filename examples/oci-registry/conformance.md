# Conformance

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
