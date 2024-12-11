.DEFAULT_GOAL:=help

.PHONY: build-ui run-ui

NPM ?= yarn

##@ Helpers

help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_\-.*]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

##@ Building

build-ui: ## Build the UI from source
	@$(NPM) install
	@$(NPM) run turbo:build

##@ Running

run-ui: ## Run UI from source
	@$(NPM) install
	@$(NPM) run turbo:dev

yarn-upgrade-stable:
	@$(NPM) set version stable
	@$(NPM) dlx @yarnpkg/sdks vscode
