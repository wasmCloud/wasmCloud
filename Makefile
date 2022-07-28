# wasmcloud/weld top-level Makefile
#
# Makefiles in this repository assume you have GNU Make (version 4.x)
#    If you're on mac, `brew install make`
#    and ensure `/usr/local/opt/make/libexec/gnubin` is in your PATH before /usr/bin

subdirs = codegen macros rpc-rs

all build release clean test update lint validate rust-check::
	for dir in $(subdirs); do \
		$(MAKE) -C $$dir $@ ; \
	done

gen:
	$(MAKE) -C codegen release
	(cd codegen && target/release/codegen)
	(cd rpc-rs && ../codegen/target/release/codegen)

#WELD_SRC := bin/Cargo.toml bin/src/*.rs codegen/Cargo.toml codegen/templates/*.toml \
#			codegen/templates/*.hbs codegen/templates/rust/*.hbs
#target/debug/weld: $(WELD_SRC)
#	cargo build --package weld-bin

.PHONY: all build release clean test
.NOTPARALLEL:
