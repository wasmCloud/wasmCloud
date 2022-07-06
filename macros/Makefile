# weld/macros/Makefile

all: build

build clean update:
	cargo $@

release:
	cargo build --release

test::
	cargo test --all-features --all-targets -- --nocapture

rust-check::
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs

.PHONY: all build release clean lint validate test update rust-check
