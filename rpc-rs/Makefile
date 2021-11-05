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

ifeq ($(shell docker ps | grep --regexp "nats:2.*/nats-server.* Up "),)
test::
	docker run --rm -d --name rpc_test_nats -p 127.0.0.1:4222:4222 nats:2.6 -js
	cargo test -- --nocapture
	docker stop rpc_test_nats
else
test::
	cargo test -- --nocapture
endif

test::
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs

.PHONY: all build release clean lint validate
