# capability-provider/blobstore-s3/Makefile

PROJECT       = blobstore-s3
CAPABILITY_ID = wasmcloud:blobstore
VENDOR        = "wasmcloud"
VERSION       = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION      = 0
oci_url       = localhost:5000/v2/$(PROJECT):$(VERSION)
NAME          = "Blobstore for AWS S3"
# builds are 'release'
# If using debug builds, change bin_path in provider_test_config.toml
RELEASE := --release

all::
	cargo build --release

include ../build/makefiles/provider.mk

run:
	killall wasmcloud_blobstore || true
	$(MAKE) all push start && sleep 3

ifeq ($(shell nc -zt -w1 127.0.0.1 4222 || echo fail),fail)
test::
	@killall blobstore-s3 || true
	docker run --rm -d --name s3-provider-test -p 127.0.0.1:4222:4222 nats:2.8.4-alpine -js
	RUST_BACKTRACE=1 RUST_LOG=debug cargo test $(RELEASE) -- --nocapture
	docker stop s3-provider-test
else
test::
	@killall blobstore-s3 || true
	cargo test $(RELEASE) -- --nocapture
endif

