# capability-providers/kafka/Makefile

PROJECT = messaging_kafka
CAPABILITY_ID = wasmcloud:messaging
VENDOR   = "wasmCloud"
NAME = "Kafka Messaging"
VERSION  = $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[] .version' | head -1)
REVISION = 0
oci_url  = localhost:5000/v2/$(PROJECT):$(VERSION)

include ../build/makefiles/provider.mk

ifeq ($(shell nc -czt -w1 127.0.0.1 4222 || echo fail),fail)
test::
	docker run --rm -d --name nats-provider-test -p 127.0.0.1:4222:4222 nats:2.8 -js
	cargo test -- --nocapture
	docker stop nats-provider-test
else
test::
	cargo test -- --nocapture
endif