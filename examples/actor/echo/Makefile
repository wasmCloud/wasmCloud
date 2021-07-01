COLOR ?= always # Valid COLOR options: {always, auto, never}
CARGO = cargo --color $(COLOR)
TARGET = target/wasm32-unknown-unknown
DEBUG = $(TARGET)/debug
RELEASE = $(TARGET)/release
KEYDIR ?= .keys
VERSION = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 8

.PHONY: all build check clean doc test update

all: release

build:
	@$(CARGO) build
	wash claims sign $(DEBUG)/echo.wasm -c wasmcloud:httpserver --name "echo" --ver $(VERSION) --rev $(REVISION)

check:
	@$(CARGO) check

clean:
	@$(CARGO) clean

doc:
	@$(CARGO) doc

test: build
	@$(CARGO) test

update:
	@$(CARGO) update

release:
	@$(CARGO) build --release
	wash claims sign $(RELEASE)/echo.wasm -c wasmcloud:httpserver --name "echo" --ver $(VERSION) --rev $(REVISION)

actor_id: release
	wash claims inspect $(RELEASE)/echo_s.wasm -o json | jq .module | tr -d "\""

run:
	export ECHO_ACTOR=$(shell make actor_id 2>&1 | tail -1)
	RUST_BACKTRACE=1 RUST_LOG=debug wasmcloud -m manifest.yaml --allowed-insecure localhost:5000
