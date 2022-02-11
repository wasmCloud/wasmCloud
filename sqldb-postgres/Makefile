# Makefile for capability-providers/sqldb-postgres

PROJECT = sqldb-postgres
CAPABILITY_ID = "wasmcloud:sqldb"
NAME = "Postgres SQL"
VENDOR = "wasmcloud"
VERSION = 0.1.0
REVISION = 0

include ../build/makefiles/provider.mk

all::
	cd examples/check-uri && cargo build

test:
	# start postgres docker container on unique 5433 in case another is running
	docker run -d --rm -it --name pgdb -p 127.0.0.1:5433:5432 -e POSTGRES_PASSWORD=postgres postgres:13
	sleep 3
	RUST_BACKTRACE=1 cargo test -- --nocapture
	docker stop pgdb

