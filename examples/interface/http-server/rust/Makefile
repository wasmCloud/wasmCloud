# http-server/rust/Makefile

.PHONY: all build clean test

all: build

build:
	cargo build

release:
	cargo build --release

clean:
	cargo clean

test: build
	cargo test
