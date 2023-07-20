# capability-provider/httpserver-rs/Makefile

# short name that will be used for the executable name
PROJECT  = httpserver
CAPABILITY_ID = wasmcloud:httpserver
VENDOR   = "wasmCloud"
NAME     = "HTTP Server"
VERSION  = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
# During development iterations, REVISION should be incremented each build
REVISION = 0
oci_url  = localhost:5000/v2/$(PROJECT):$(VERSION)
oci_insecure = --insecure

include ../build/makefiles/provider.mk

ifeq ($(shell nc -zt -w1 127.0.0.1 4222 || echo fail),fail)
test::
	docker run --rm -d --name nats-provider-test -p 127.0.0.1:4222:4222 nats:2.9 -js
	cargo test -- --nocapture
	docker stop nats-provider-test
else
test::
	cargo test -- --nocapture
endif

