# sqldb-dynamodb Makefile

CAPABILITY_ID = "wasmcloud:sqldb_dynamodb"
NAME = "SQLDB for AWS DynamoDB"
VENDOR = "wasmcloud"
PROJECT = sqldb-dynamodb
VERSION = 0.1.0
REVISION = 0

include ../build/makefiles/provider.mk

all::
	cargo build --release

test::
	cargo clippy --all-targets --all-features
	docker-compose -f tests/docker-compose.yaml up &
	RUST_BACKTRACE=1 RUST_LOG=debug cargo test $(RELEASE) -- --nocapture
	docker-compose -f tests/docker-compose.yaml down

