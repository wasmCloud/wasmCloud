# rpc-rs/Makefile

all: build

build:
	cargo build

release:
	cargo build --release

clean:
	cargo clean

# Run lint check on all smithy models in the models/smithy folder
lint:
	$(WELD) lint

# Run validation checks on all smithy models in the models/smithy folder
validate:
	$(WELD) validate

ifeq ($(shell nc -czt -w1 127.0.0.1 4222 || echo fail),fail)
test::
	docker run --rm -d --name wasmbus-rpc-test -p 127.0.0.1:4222:4222 nats:2 -js
	cargo test -- --nocapture
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs
	docker stop wasmbus-rpc-test
else
test::
	cargo test -- --nocapture
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs
endif

.PHONY: all build release clean lint validate
