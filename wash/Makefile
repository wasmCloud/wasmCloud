.DEFAULT_GOAL:=help

.PHONY: build build-watch test test-integration test-all clean help

CARGO ?= cargo
DOCKER ?= docker

##@ Helpers

help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_\-.*]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

clean: ## Clean all tests
	cargo clean
	wash drain all

##@ Building

build: ## Build the project
	@$(CARGO) build

build-watch: ## Continuously build the project
	@$(CARGO) watch -x build

##@ Testing

test: ## Run unit test suite
	@$(CARGO) test --no-fail-fast --bin wash -- --nocapture
	@$(CARGO) test --no-fail-fast -p wash-lib -- --nocapture

test-integration: ## Run integration test suite
	@$(DOCKER) compose -f ./tools/docker-compose.yml up --detach
	@$(CARGO) test --no-fail-fast --test "integration*" -- --nocapture
	@$(DOCKER) compose -f ./tools/docker-compose.yml down

test-unit: ## Run one or more unit tests
ifeq ("","$(TARGET)")
	@$(CARGO) test -- --nocapture
else
	@$(CARGO) test $(TARGET) -- --nocapture
endif

test-unit-watch: ## Run tests continuously
	@$(CARGO) watch -- $(MAKE) test-unit

rust-check: ## Run rust checks
	cargo fmt --all --check
	cargo clippy --all-features --all-targets --workspace

test-all: ## Run all tests
	$(MAKE) test
	$(MAKE) test-integration
	$(MAKE) rust-check
