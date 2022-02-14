# capability-provider/blobstore-s3/Makefile

PROJECT = blobstore-s3
CAPABILITY_ID = wasmcloud:blobstore
VENDOR   = "wasmcloud"
VERSION  = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 0
oci_url  = localhost:5000/v2/$(PROJECT):$(VERSION)

include ../build/makefiles/provider.mk

ifeq ($(shell nc -czt -w1 127.0.0.1 4222 || echo fail),fail)
test::
	docker run --rm -d --name s3-provider-test -p 127.0.0.1:4222:4222 nats:2.7.2 -js
	cargo test -- --nocapture
	docker stop s3-provider-test
else
test::
	cargo test -- --nocapture
endif

