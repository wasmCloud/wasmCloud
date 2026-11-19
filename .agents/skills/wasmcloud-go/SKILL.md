---
name: wasmcloud-go
description: Test, lint, code generation, and manifest checks for wasmCloud's Go modules. Use when changing or reviewing Go code, go.mod, kubebuilder markers, CRDs, or RBAC in runtime-operator/ or runtime-gateway/, or when running operator envtest or Kind end-to-end tests.
---

# Go changes

Commands below start at the repository root unless otherwise noted.
[`runtime-operator.yml`](../../../.github/workflows/runtime-operator.yml) and
[`runtime-gateway.yml`](../../../.github/workflows/runtime-gateway.yml) are the source of truth;
if a command here differs from the workflow, follow the workflow.

- Run checks in the affected module; the operator and gateway have separate CI workflows.
- For `runtime-operator`, run `GOWORK=off make -C runtime-operator test` from the repository root.
  This generates manifests and code, formats and vets Go, and sets up envtest before testing.
  Run its `lint` target as well.
- `make test` regenerates files under `runtime-operator/config/`.
  Commit them with the source change: the workflow's `manifests` job fails if `make manifests` leaves a diff.
- After changing `+kubebuilder:rbac` markers, update the Helm chart to match,
  then run the RBAC parity script from the same `manifests` job.
- For `runtime-gateway`, run `go test -v ./...` from `runtime-gateway/`.
  Follow [runtime-gateway.yml](../../../.github/workflows/runtime-gateway.yml) for the build and lint checks.
- Operator end-to-end tests need an isolated Kind cluster and additional setup;
  follow [runtime-operator.yml](../../../.github/workflows/runtime-operator.yml),
  then run `make test-e2e` from `runtime-operator/`.
