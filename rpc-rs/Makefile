# rpc-rs/Makefile
WELD = "../target/debug/weld"

.PHONY: all build release clean lint validate

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
