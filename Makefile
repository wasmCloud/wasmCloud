# wasmcloud/weld top-level Makefile
#
# Makefiles in this repository assume you have GNU Make (version 4.x)
#    If you're on mac, `brew install make`
#    and ensure `/usr/local/opt/make/libexec/gnubin` is in your PATH before /usr/bin

subdirs = codegen macros rpc-rs

MODEL_OUTPUT := codegen/src/wasmbus_model.rs rpc-rs/src/wasmbus_model.rs
#MODEL_SRC    := examples/interface/wasmbus-core/wasmcloud-model.smithy \
#				examples/interface/wasmbus-core/codegen.toml

all build release clean test update lint validate rust-check::
	for dir in $(subdirs); do \
		$(MAKE) -C $$dir $@ ; \
	done

test::
	$(MAKE) check-model

check-model: $(MODEL_OUTPUT)
	@diff $(MODEL_OUTPUT) || (echo ERROR: Model files differ && exit 1)

gen:
	$(MAKE) -C codegen release
	(cd codegen && target/release/codegen)
	(cd rpc-rs && ../codegen/target/release/codegen)

#WELD_SRC := bin/Cargo.toml bin/src/*.rs codegen/Cargo.toml codegen/templates/*.toml \
#			codegen/templates/*.hbs codegen/templates/rust/*.hbs
#target/debug/weld: $(WELD_SRC)
#	cargo build --package weld-bin

.PHONY: all build release clean test check-model
.NOTPARALLEL:
