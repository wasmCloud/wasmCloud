# Makefile for capability-providers/sqldb-postgres

PROJECT = sqldb-postgres
CAPABILITY_ID = "wasmcloud:sqldb"
NAME = "SqlDb-Postgres"
VENDOR = "wasmcloud"
VERSION = 0.1.0
REVISION = 0

include ../build/makefiles/provider.mk

all::
	cd examples/check-uri && cargo build

test:
	RUST_BACKTRACE=1 cargo test -- --nocapture

