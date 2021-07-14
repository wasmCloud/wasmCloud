# weld top-level Makefile
#
# Makefiles in this repository assume you have GNU Make (version 4.x)
#    If you're on mac, `brew install make`
#    and ensure `/usr/local/opt/make/libexec/gnubin` is in your PATH before /usr/bin

MODEL_OUTPUT := codegen/src/wasmbus_model.rs rpc-rs/src/wasmbus_model.rs
MODEL_SRC    := examples/interface/wasmbus-core/wasmcloud-model.smithy \
				examples/interface/wasmbus-core/codegen.toml
WELD         := target/debug/weld


all build: $(MODEL_OUTPUT)
	cargo build
	$(MAKE) -C examples WELD=$(realpath $(WELD))

clean:
	cargo clean
	$(MAKE) -C examples clean

release: $(MODEL_OUTPUT)
	cargo build --release
	$(MAKE) -C examples

test:
	cargo $@

$(MODEL_OUTPUT): $(WELD) $(MODEL_SRC)
	$(WELD) gen --config examples/interface/wasmbus-core/codegen.toml

check-model: $(MODEL_OUTPUT)
	@diff $(MODEL_OUTPUT) || (echo ERROR: Model files differ && exit 1)

WELD_SRC := bin/Cargo.toml bin/src/*.rs codegen/Cargo.toml codegen/templates/*.toml \
			codegen/templates/*.hbs codegen/templates/rust/*.hbs
target/debug/weld: $(WELD_SRC)
	cargo build --package weld-bin

.PHONY: all build release clean test
.NOTPARALLEL:
