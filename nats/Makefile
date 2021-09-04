# capability-provider/nats/Makefile

PROJECT = nats_messaging
CAPABILITY_ID = wasmcloud:messaging
VENDOR   = "wasmcloud"
VERSION  = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 0
oci_url  = localhost:5000/v2/$(PROJECT):$(VERSION)

include ../build/makefiles/provider.mk

all::
	cargo build --tests

test:
	RUST_BACKTRACE=1 RUST_LOG=debug cargo test -- --nocapture

