---
name: wasmcloud-go
description: Implement, review, or validate Go changes in runtime-operator or runtime-gateway, using the module-specific tests, lint, generation, and envtest setup.
---

# Go changes

Commands below start at the repository root unless otherwise noted.

- Run checks in the affected module; the operator and gateway have separate CI workflows.
- For `runtime-operator`, run `GOWORK=off make -C runtime-operator test` from the repository root.
  This generates manifests and code, formats and vets Go, and sets up envtest before testing.
  Run its `lint` target as well.
- For `runtime-gateway`, run `go test -v ./...` from `runtime-gateway/`.
  Follow [runtime-gateway.yml](../../../.github/workflows/runtime-gateway.yml) for the build and lint checks.
- Operator end-to-end tests need an isolated Kind cluster and additional setup;
  follow [runtime-operator.yml](../../../.github/workflows/runtime-operator.yml),
  then run `make test-e2e` from `runtime-operator/`.
