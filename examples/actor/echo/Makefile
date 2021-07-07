COLOR ?= always # Valid COLOR options: {always, auto, never}
CARGO := cargo --color $(COLOR)
KEYDIR ?= .keys
NAME := "echo"
REVISION := 8
TARGET := target/wasm32-unknown-unknown
VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)


all: release

build:
	@$(CARGO) build
	wash claims sign $(TARGET)/debug/$(NAME).wasm -c wasmcloud:httpserver --name "$(NAME)" --ver $(VERSION) --rev $(REVISION)

check:
	@$(CARGO) check

clean:
	@$(CARGO) clean

doc:
	@$(CARGO) doc

test:
	@$(CARGO) test

update:
	@$(CARGO) update

release:
	@$(CARGO) build --release
	wash claims sign $(TARGET)/release/$(NAME).wasm -c wasmcloud:httpserver --name "$(NAME)" --ver $(VERSION) --rev $(REVISION)

actor_id: release
	wash claims inspect $(TARGET)/release/$(NAME)_s.wasm -o json | jq .module | tr -d "\""

run:
	export ACTOR_ID=$(shell make actor_id 2>&1 | tail -1)
	RUST_BACKTRACE=1 RUST_LOG=debug wasmcloud -m manifest.yaml --allowed-insecure localhost:5000

.PHONY: all build release check clean doc test update
