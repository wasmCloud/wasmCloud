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

include ../build/makefiles/provider.mk

test:
	cargo test -- --nocapture
