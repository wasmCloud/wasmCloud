# weld/rpc-rs/Makefile

all: build

build clean update:
	cargo $@

release:
	cargo build --release

# Run lint check on all smithy models
lint validate:
	wash $@

ifeq ($(shell nc -czt -w1 127.0.0.1 4222 || echo fail),fail)
test::
	docker run --rm -d --name wasmbus-rpc-test -p 127.0.0.1:4222:4222 nats:2 -js
	cargo test -- --nocapture
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs
	docker stop wasmbus-rpc-test
else
test::
	WASMBUS_RPC_TIMEOUT_MS=4000 cargo test -- --nocapture
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs
endif

.PHONY: all build release clean lint validate test update rust-check
