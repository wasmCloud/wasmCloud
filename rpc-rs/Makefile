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

test::
	cargo test -- --nocapture

test::
	cargo clippy --all-features --all-targets
	rustup default nightly
	rustfmt --edition 2021 --check src/*.rs

.PHONY: all build release clean lint validate
