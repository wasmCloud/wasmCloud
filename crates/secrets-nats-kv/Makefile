.PHONY: test-integration

CARGO ?= cargo
DOCKER ?= docker

test-integration: ## Run the entire integration test suite (with docker compose)
	@$(DOCKER) compose -f ./tools/docker-compose.yml up --detach
	@$(CARGO) nextest run $(TARGET) --profile integration -E 'kind(test)' --nocapture
	@$(DOCKER) compose -f ./tools/docker-compose.yml down

test-integration-ci: ## Run the entire integration test suite only
	@$(CARGO) nextest run --profile ci -E 'kind(test)' --nocapture
