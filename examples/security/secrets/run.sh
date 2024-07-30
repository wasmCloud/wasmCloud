#!/bin/bash

echo "Starting docker compose infrastructure ..."
subject_base=wasmcloud.secrets
encryption_key=$(nk -gen x25519)
transit_key=$(nk -gen x25519)
ENCRYPTION_XKEY_SEED=$encryption_key TRANSIT_XKEY_SEED=$transit_key docker compose up -d

sleep 5
echo "Putting secrets and mappings in NATS KV ..."
component_key=$(wash inspect ./component-keyvalue-counter-auth/build/component_keyvalue_counter_auth_s.wasm -o json | jq -r '.component')
provider_key=$(wash inspect ./provider-keyvalue-redis-auth/build/wasmcloud-example-auth-kvredis.par.gz -o json | jq -r '.service')
pushd ../../../crates/secrets-nats-kv > /dev/null
TRANSIT_XKEY_SEED=$transit_key cargo run -- put api_password --string opensesame
TRANSIT_XKEY_SEED=$transit_key cargo run -- put redis_password --string sup3rS3cr3tP4ssw0rd
TRANSIT_XKEY_SEED=$transit_key cargo run -- put default_redis_password --string sup3rS3cr3tP4ssw0rd
cargo run -- add-mapping $component_key --secret api_password
cargo run -- add-mapping $provider_key --secret redis_password --secret default_redis_password
popd > /dev/null

echo "Starting wasmCloud ..."
pushd ../../../ > /dev/null
cargo run -- --secrets-topic $subject_base \
    --allow-file-load &

popd > /dev/null


echo "Waiting for wasmCloud to start ..."
while [ "$(wash get hosts -o json | jq '.hosts')" == "[]" ]; do
    sleep 1
done

host_id=$(wash get hosts -o json | jq -r '.hosts[0].id')
echo "Starting authenticated provider and component on host $host_id ..."
wash drain lib
# Start component
wash config put SECRET_api_password key=api_password backend=nats-kv
wash start component file://$(pwd)/component-keyvalue-counter-auth/build/component_keyvalue_counter_auth_s.wasm kvcounter-auth \
    --host-id $host_id \
    --config SECRET_api_password \
    --max-instances 100

# Link for HTTP and keyvalue
wash config put SECRET_redis_password key=redis_password backend=nats-kv
wash config put redis_url url=127.0.0.1:6379
wash link put kvcounter-auth kvredis-auth wasi keyvalue \
    --interface atomics \
    --target-config SECRET_redis_password \
    --target-config redis_url
wash config put default_http address=0.0.0.0:8080
wash link put http-server kvcounter-auth wasi http \
    --interface incoming-handler \
    --source-config default_http

# Start providers
wash config put SECRET_default_redis_password key=default_redis_password backend=nats-kv
wash start provider file://$(pwd)/provider-keyvalue-redis-auth/build/wasmcloud-example-auth-kvredis.par.gz kvredis-auth \
    --host-id $host_id \
    --config SECRET_default_redis_password
wash start provider ghcr.io/wasmcloud/http-server:0.22.0 http-server

sleep 5

# A separate piece of the link requirement is to be able to send a link with a secret _after_ the provider is started
# and not transmit the secret in plaintext
wash link put other-kvcounter-auth kvredis-auth wasi keyvalue \
    --interface atomics \
    --target-config SECRET_redis_password \
    --target-config redis_url

echo "Now send requests to localhost:8080 ..."

# Neat little trick to wait for CTRL+c to exit
cleanup() {
    echo "CTRL+c detected, cleaning up ..."
    docker compose down
}
trap cleanup INT

sleep 999999
