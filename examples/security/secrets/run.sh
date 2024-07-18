#!/bin/bash

echo "Starting docker compose infrastructure ..."
subject_base=wasmcloud.secrets
encryption_key=$(nk -gen x25519)
transit_key=$(nk -gen x25519)
ENCRYPTION_KEY=$encryption_key TRANSIT_KEY=$transit_key SUBJECT_BASE=$subject_base docker compose up -d

echo "Putting secrets and mappings in NATS KV ..."
sleep 5
pushd ./secret-setup
cargo run -- $transit_key
popd > /dev/null
component_key=$(wash inspect ./component-keyvalue-counter-auth/build/component_keyvalue_counter_auth_s.wasm -o json | jq -r '.component')
component_mapping="[\"api_password\"]"
provider_key=$(wash inspect ./provider-keyvalue-redis-password/build/wasmcloud-example-auth-kvredis.par.gz -o json | jq -r '.service')
provider_mapping="[\"redis_password\", \"default_redis_password\"]"
nats req "$subject_base.v0.nats-kv.add_mapping.$provider_key" "$provider_mapping"
nats req "$subject_base.v0.nats-kv.add_mapping.$component_key" "$component_mapping"

echo "Starting wasmCloud ..."
pushd ../../../ > /dev/null
cargo run -- --secrets-topic $subject_base \
    --allow-file-load \
    --log-level debug &

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
wash config put SECRET_api_password key=redis_password backend=nats-kv
wash config put redis_url url=127.0.0.1:6379
wash link put kvcounter-auth kvredis-auth wasi keyvalue \
    --interface atomics \
    --target-config SECRET_api_password \
    --target-config redis_url
wash config put default_http address=0.0.0.0:8080
wash link put http-server kvcounter-auth wasi http \
    --interface incoming-handler \
    --source-config default_http

# Start providers
wash config put SECRET_default_redis_password key=default_redis_password backend=nats-kv
wash start provider file://$(pwd)/provider-keyvalue-redis-password/build/wasmcloud-example-auth-kvredis.par.gz kvredis-auth \
    --host-id $host_id \
    --config SECRET_default_redis_password
wash start provider ghcr.io/wasmcloud/http-server:0.21.0 http-server

sleep 5

# A separate piece of the link requirement is to be able to send a link with a secret _after_ the provider is started
# and not transmit the secret in plaintext
wash link put other-kvcounter-auth kvredis-auth wasi keyvalue \
    --interface atomics \
    --target-config SECRET_api_password \
    --target-config redis_url

echo "Now send requests to localhost:8080 ..."

# Neat little trick to wait for CTRL+c to exit
cleanup() {
    echo "CTRL+c detected, cleaning up ..."
    docker compose down
}
trap cleanup INT

sleep 999999
