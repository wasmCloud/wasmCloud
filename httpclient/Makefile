# capability-provider/httpclient/Makefile

CAPABILITY_ID = wasmcloud:httpclient
NAME = "HttpClient"
VENDOR = "wasmcloud"
PROJECT = httpclient
VERSION = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 0

include ../build/makefiles/provider.mk

test:
	cargo test -- --nocapture

