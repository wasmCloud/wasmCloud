# capability-provider/kv-vault/Makefile

PROJECT       = kv-vault
CAPABILITY_ID = wasmcloud:keyvalue
VENDOR        = "wasmCloud"
NAME          = "KeyValue: Hashicorp Vault"
VERSION       = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION      = 0
oci_url       = localhost:5000/v2/$(PROJECT):$(VERSION)

include ../build/makefiles/provider.mk

test::
	bash ./run-test.sh

test::
	rustfmt --edition 2021 --check src/*.rs
	cargo clippy --all-features --all-targets