# examples/actor/hello_send

COLOR ?= always # Valid COLOR options: {always, auto, never}
CARGO := cargo --color $(COLOR)
KEYDIR ?= .keys
REVISION := 8
RUST_DEPS := Cargo.toml Makefile $(wildcard src/*.rs) .cargo/config.toml
TARGET := target/wasm32-unknown-unknown
VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)

FNAME := hello_send
DIST_WASM := build/$(FNAME)_s.wasm
DIST_EXS := build/exs.out

all: $(DIST_WASM) $(DIST_EXS)

$(DIST_WASM): $(TARGET)/release/$(FNAME)_s.wasm
	@mkdir -p $(dir $@)
	cp -p $< $@

.ONESHELL:
$(DIST_EXS): $(DIST_WASM)
	@mkdir -p $(dir $@)
	@cat <<- EOF > $@
	%{
		name: "$(FNAME)",
		path: "$(abspath $(DIST_WASM))",
	    key: "$(shell wash claims inspect $(DIST_WASM) -o json | jq -r .module )",
	},
	EOF

$(TARGET)/debug/$(FNAME).wasm: $(RUST_DEPS)
	$(CARGO) build

$(TARGET)/release/$(FNAME).wasm: $(RUST_DEPS)
	$(CARGO) build --release

# sign it (debug or release)
%/$(FNAME)_s.wasm: %/$(FNAME).wasm
	wash claims sign $< \
		-c wasmcloud:httpserver \
		-c wasmcloud:example:hello \
		--name "$(FNAME)" --ver $(VERSION) --rev $(REVISION)

check clippy doc test update:
	$(CARGO) $@

clean:
	$(CARGO) clean
	rm -rf build

claims: $(DIST_WASM)
	wash claims inspect $(DIST_WASM)

actor_id: $(DIST_WASM)
	wash claims inspect $(DIST_WASM) -o json | jq -r .module

.PHONY: check clean doc test update

