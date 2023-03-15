.PHONY: build

CARGO ?= cargo

build: ## Build the project
	$(CARGO) build

build-watch: ## Build the project (continuously)
	$(CARGO) watch -- $(MAKE) build
