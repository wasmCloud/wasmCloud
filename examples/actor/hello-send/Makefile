COLOR ?= always # Valid COLOR options: {always, auto, never}
CARGO = cargo --color $(COLOR)
TARGET = target/wasm32-unknown-unknown
DEBUG = $(TARGET)/debug
RELEASE = $(TARGET)/release
KEYDIR ?= .keys
VERSION = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 8
RUST_DEPS = Cargo.toml Makefile $(wildcard src/*.rs) .cargo/config.toml

FNAME = hello_send
DIST_WASM = build/$(FNAME)_s.wasm
DIST_EXS = build/exs.out

.PHONY: check clean doc test update

all: $(DIST_WASM) $(DIST_EXS)

$(DIST_WASM): $(RELEASE)/$(FNAME)_s.wasm
	@mkdir -p $(dir $@)
	cp -p $< $@

.ONESHELL:
$(DIST_EXS): $(DIST_WASM)
	@mkdir -p $(dir $@)
	@cat <<- EOF > $@
	%{
		:name => "$(FNAME)",
		:path => "$(abspath $(DIST_WASM))",
	    :key => "$(shell wash claims inspect $(DIST_WASM) -o json | jq -r .module )",
	},
	EOF

$(DEBUG)/$(FNAME).wasm: $(RUST_DEPS)
	@$(CARGO) build

$(RELEASE)/$(FNAME).wasm: $(RUST_DEPS)
	@$(CARGO) build --release

# sign it (debug or release)
%/$(FNAME)_s.wasm: %/$(FNAME).wasm
	wash claims sign $< \
		-c wasmcloud:httpserver \
		-c wasmcloud:example:hello \
		--name "$(FNAME)" --ver $(VERSION) --rev $(REVISION)

check clean clippy doc test update:
	@$(CARGO) $@

claims: $(DIST_WASM)
	wash claims inspect $(DIST_WASM)

actor_id: $(DIST_WASM)
	wash claims inspect $(DIST_WASM) -o json | jq -r .module

#run: $(DIST_WASM)
#	export ACTOR_ID=$(shell make actor_id 2>&1 | tail -1)
#	RUST_BACKTRACE=1 RUST_LOG=debug wasmcloud -m manifest.yaml --allowed-insecure localhost:5000
