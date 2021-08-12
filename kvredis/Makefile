# capability-provider/redis/Makefile

CAPABILITY_ID = wasmcloud:keyvalue
NAME = "KVRedis"
VENDOR = "wasmCloud"
PROJECT = kvredis
# first two-parts of the semver version
VERSION = 0.2
# the tag in the registry will be VERSION.REVISION, and must be incremented
# any time the binary changes.
# During development iterations, REVISION should be incremented each build
REVISION = 0

# generate par file with release build and build tests
all:
	cargo build --bins --tests

test:
	killall target/debug/kvredis
	RUST_BACKTRACE=1 cargo test -- --nocapture

include ../build/makefiles/provider.mk

