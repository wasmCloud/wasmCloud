.DEFAULT_GOAL:=help

.PHONY: build build-watch test test-integration test-all clean help

CARGO ?= cargo
DOCKER ?= docker
PYTHON ?= python3

##@ Helpers

help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_\-.*]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

clean: ## Clean all tests
	@$(CARGO) clean
	wash drain all

deps-check:
	@$(PYTHON) tools/deps_check.py

##@ Building

build: ## Build the project
	@$(CARGO) build

build-watch: ## Continuously build the project
	@$(CARGO) watch -x build

##@ Testing

test: ## Run the entire unit test suite
	@$(CARGO) nextest run --no-fail-fast -p wash-lib
	@$(CARGO) nextest run --no-fail-fast --bin wash

test-integration: ## Run the entire integration test suite
	@$(DOCKER) compose -f ./tools/docker-compose.yml up --detach
	@$(CARGO) nextest run --profile integration -E 'kind(test)'
	@$(DOCKER) compose -f ./tools/docker-compose.yml down

test-watch: ## Run unit tests continously, can optionally specify a target test filter.
	@$(CARGO) watch -- $(CARGO) nextest run $(TARGET)

rust-check: ## Run rust checks
	@$(CARGO) fmt --all --check
	@$(CARGO) clippy --all-features --all-targets --workspace

test-all: ## Run all tests
	$(MAKE) test
	$(MAKE) test-integration
	$(MAKE) rust-check
