# capability-provider/redis/Makefile

PROJECT = kvredis
CAPABILITY_ID = wasmcloud:keyvalue
VENDOR   = "wasmCloud"
NAME     = "Redis KeyValue Store"
VERSION  = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 0
oci_url  = localhost:5000/v2/$(PROJECT):$(VERSION)

include ../build/makefiles/provider.mk

test:
	killall target/debug/kvredis || true
	RUST_BACKTRACE=1 cargo test -- --nocapture
