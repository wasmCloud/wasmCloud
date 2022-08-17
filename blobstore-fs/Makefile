# blobstore-fs Makefile

CAPABILITY_ID = "wasmcloud:blobstore"
NAME = "Blobstore for unix and windows file system"
VENDOR = "wasmcloud"
PROJECT = blobstore_fs
VERSION  = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 0

# builds are 'release'
# If using debug builds, change bin_path in provider_test_config.toml
TEST_FLAGS := --release -- --nocapture

include ../build/makefiles/provider.mk

test::
	cargo test $(TEST_FLAGS)

