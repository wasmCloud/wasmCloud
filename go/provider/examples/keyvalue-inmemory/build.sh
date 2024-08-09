#!/bin/bash

# Build the Go binary and package it into a par file
# This is designed to be run from the root of the project
go generate ./...
go build ./
wash par create --vendor wasmcloud --name "KeyValue Go" --binary ./keyvalue-inmemory --compress
rm keyvalue-inmemory