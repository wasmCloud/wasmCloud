# capability-provider/redis/Makefile

CAPABILITY_ID = wasmcloud:keyvalue
NAME = "KVRedis"
VENDOR = "wasmCloud"
PROJECT = kvredis
VERSION = 0.2.0
REVISION = 0

# generate par file with release build and build tests
all:
	cargo build --bins --tests

test:
	killall target/debug/kvredis
	RUST_BACKTRACE=1 cargo test -- --nocapture

include ../build/makefiles/provider.mk

