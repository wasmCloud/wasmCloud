all: proto

# protobuf
.PHONY: proto
proto: proto-generate

.PHONY: proto
proto-generate:
	docker run --volume "$(PWD):/workspace" --workdir /workspace bufbuild/buf generate
	
.PHONY: proto-format
proto-format:
	docker run --volume "$(PWD):/workspace" --workdir /workspace bufbuild/buf format -w

.PHONY: proto-lint
proto-lint:
	docker run --volume "$(PWD):/workspace" --workdir /workspace bufbuild/buf lint

# Local Dev
.PHONY: kind-setup
kind-setup:
	kind create cluster --name wasmcloud --config "deploy/kind/kind-config.yaml"

.PHONY: kind-nuke
kind-nuke:
	kind delete cluster --name wasmcloud

# Helm
.PHONY: helm-build
helm-build:
	helm dependency build charts/runtime-operator

.PHONY: helm-render
helm-render:
	helm template -n example-ns example-name charts/runtime-operator

.PHONY: helm-install
helm-install:
	helm upgrade --install --create-namespace -n wasmcloud-system -f charts/runtime-operator/values.local.yaml operator-dev charts/runtime-operator

.PHONY: helm-uninstall
helm-uninstall:
	helm delete -n wasmcloud-system --ignore-not-found --cascade foreground operator-dev

