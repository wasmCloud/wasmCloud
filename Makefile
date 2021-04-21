.DEFAULT_GOAL:=help

##@ Testing

test: ## Run unit test suite
	cargo test --no-fail-fast --verbose --bin wash -- --nocapture

test-integration-wasm3: ##Run integration test suite with wasm3 engine
	docker-compose -f ./tools/docker-compose.yml up --detach
	cargo test --no-fail-fast --verbose --test "integration*" --no-default-features --features wasm3 -- --nocapture
	docker-compose -f ./tools/docker-compose.yml down

test-integration-wasmtime: ##Run integration test suite with wasmtime engine
	docker-compose -f ./tools/docker-compose.yml up --detach
	cargo test --no-fail-fast --verbose --test "integration*" --no-default-features --features wasmtime -- --nocapture
	docker-compose -f ./tools/docker-compose.yml down

test-all: test test-integration-wasm3 test-integration-wasmtime ## Run all tests

##@ Helpers

clean:
	wash drain all

help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_\-.*]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)
