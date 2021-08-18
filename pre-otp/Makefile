.DEFAULT_GOAL:=help

PROVIDERS = fs http-client http-server logging nats redis redisgraph redis-streams s3 telnet


##@ Building

.PHONY: all $(PROVIDERS)

all: $(PROVIDERS) ## Build all capability providers

$(PROVIDERS):
	$(MAKE) -C $@ par

##@ Helpers

.PHONY: help

copy-common: ## Copy common build files
	@for f in $(PROVIDERS); \
	do \
	    cp Cross.toml $$f/Cross.toml && \
	    cp Makefile.common $$f/Makefile; \
	done

help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_\-.*]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)
