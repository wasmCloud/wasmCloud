#!/bin/bash

echo "Building NATS KV setup utility..."
pushd ./secret-setup
cargo build
popd

echo "Building wasmCloud ..."
pushd ../../../
cargo build
popd

echo "Building application component ..."
wash build -p component-keyvalue-counter-auth

echo "Building application provider ..."
wash build -p provider-keyvalue-redis-auth