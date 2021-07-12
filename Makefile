# weld top-level Makefile
#
# Makefiles in this repository assume you have GNU Make (version 4.x)
#    If you're on mac, `brew install make`
#    and ensure `/usr/local/opt/make/libexec/gnubin` is in your PATH before /usr/bin

MODEL_OUTPUT := codegen/src/wasmbus_model.rs rpc-rs/src/wasmbus_model.rs
MODEL_SRC    := examples/interface/wasmbus-core/wasmcloud-model.smithy \
				examples/interface/wasmbus-core/codegen.toml

all build: $(MODEL_OUTPUT)
	cargo build
	$(MAKE) -C examples

clean:
	cargo clean
	$(MAKE) -C examples clean

release:: $(MODEL_OUTPUT)
	cargo build --release
	$(MAKE) -C examples

test::
	cargo $@

$(MODEL_OUTPUT) : $(MODEL_SRC) $(WELD_D)
	$(WELD_D) gen --config examples/interface/wasmbus-core/codegen.toml

check-model: $(MODEL_OUTPUT)
	@diff $(MODEL_OUTPUT) || (echo ERROR: Model files differ && exit 1)

WELD_SRC := bin/Cargo.toml bin/src/*.rs codegen/Cargo.toml codegen/src/*.rs codegen/templates/*.toml \
			codegen/templates/*.hbs codegen/templates/rust/*.hbs
WELD_D   := target/debug/weld
$(WELD_D): $(WELD_SRC)
	cargo build --package weld-bin

#$(WELD_R): bin/Cargo.toml bin/src/*.rs codgen/**
#	cargo build --release --package weld-bin

.PHONY: all build release clean test
.NOTPARALLEL:
