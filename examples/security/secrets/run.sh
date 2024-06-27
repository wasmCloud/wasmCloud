#!/bin/bash

echo "Starting Redis with password ..."
redis-server ./redis.conf > /dev/null 2>&1 &
redis_pid=$!

echo "Starting NATS server ..."
nats-server -js > /dev/null 2>&1 &
nats_pid=$!

echo "Starting NATS KV secrets backend ..."
subject_base=wasmcloud.secrets
encryption_key=$(nk -gen x25519)
transit_key=$(nk -gen x25519)
pushd ../../../crates/secrets-nats-kv > /dev/null
cargo run -- --encryption-xkey-seed $encryption_key \
    --transit-xkey-seed $transit_key \
    --subject-base $subject_base \
    --secrets-bucket WASMCLOUD_EXAMPLE_SECRETS_default &
nats_kv_pid=$!

popd > /dev/null

echo "Putting secrets and mappings in NATS KV ..."
sleep 5
pushd ./secret-setup
cargo run -- $transit_key
popd > /dev/null
component_key=$(wash inspect ./component-keyvalue-counter-auth/build/component_keyvalue_counter_auth_s.wasm -o json | jq -r '.component')
component_mapping="[\"api_password\"]"
provider_key=$(wash inspect ./provider-keyvalue-redis-password/build/wasmcloud-example-auth-kvredis.par.gz -o json | jq -r '.service')
provider_mapping="[\"redis_password\"]"
nats req "$subject_base.v0.nats-kv.add_mapping.$provider_key" "$provider_mapping"
nats req "$subject_base.v0.nats-kv.add_mapping.$component_key" "$component_mapping"

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
    --config SECRET_api_password

# Link for HTTP and keyvalue
wash link put kvcounter-auth kvredis-auth wasi keyvalue --interface atomics
wash config put default_http address=0.0.0.0:8080
wash link put http-server kvcounter-auth wasi http \
    --interface incoming-handler \
    --source-config default_http

# Start providers
wash config put SECRET_redis_password key=redis_password backend=nats-kv
wash start provider file://$(pwd)/provider-keyvalue-redis-password/build/wasmcloud-example-auth-kvredis.par.gz kvredis-auth \
    --host-id $host_id \
    --config SECRET_redis_password
wash start provider ghcr.io/wasmcloud/http-server:0.21.0 http-server

echo "Now send requests to localhost:8080 ..."

# Neat little trick to wait for CTRL+c to exit
cleanup() {
    echo "CTRL+c detected, cleaning up ..."
    wash stop host $host_id
    kill $nats_kv_pid
    kill $redis_pid
    kill $nats_pid
}
trap cleanup INT

sleep 999999
