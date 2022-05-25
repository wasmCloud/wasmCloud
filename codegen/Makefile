# weld/codegen/Makefile

all: build

build clean update::
	cargo $@

clean::
    $(shell test -d examples/iface/rust && cd examples/iface/rust && cargo clean)

release:
	cargo build --release

# Run lint check on all smithy models
lint validate:
	wash $@

test::
	cargo test --all-features --all-targets -- --nocapture

rust-check::
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs bin/*.rs

.PHONY: all build release clean lint validate test update rust-check
