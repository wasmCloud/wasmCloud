#!/usr/bin/env bash
set -e

# This script starts vault in dev mode in a docker container, and runs cargo tests
if ! nc -z 127.0.0.1 4222; then
  echo "Unable to connect to NATS on port 4222, ensure it's running with jetstream before running tests"
  exit 1
fi

# localhost port for server - should be unique to avoid conflicts
PORT=11182
# Name of environment variables file
ENV_FILE=vault_test.env
# name of vault's temporary docker container
CONTAINER_NAME=kv-vault-test
# mount point, default is "secret"
VAULT_MOUNT=secret
# debug setting for rust test code
RUST_LOG=debug
# build release flag, empty for debug, or '--release' for release build
# If changed, make sure it's consistent with 'bin_path' in provider_test_config.toml
RELEASE_FLAG=--release

cleanup() {
    rm -f ${ENV_FILE}
    docker rm -f ${CONTAINER_NAME} 2>/dev/null
    killall -q kv-vault || true
}

# make sure it's built
cargo build ${RELEASE_FLAG} --all-features --all-targets
cleanup

# start vault docker in dev mode, no tls
# scrape container log to get Root Token
docker run --rm -d \
  --cap-add=IPC_LOCK \
  --name ${CONTAINER_NAME} \
  -p 127.0.0.1:${PORT}:8200 \
  vault:1.13.3
sleep 2
export VAULT_TOKEN="$(docker logs ${CONTAINER_NAME} 2>&1 | grep 'Root Token:' | sed -E 's/Root Token: //')"

# enable secrets engine
# secret/ is mounted automatically so --path arg should only be used if VAULT_MOUNT is something else
[[ -n "$VAULT_MOUNT" ]] && [[ "$VAULT_MOUNT" != "secret" ]] && [[ "$VAULT_MOUNT" != "secret/" ]] && PATH_ARG=-path=$VAULT_MOUNT
docker exec -i -e VAULT_TOKEN=${VAULT_TOKEN} ${CONTAINER_NAME} \
    vault secrets enable -version=2 -local \
        -address=http://127.0.0.1:8200 $PATH_ARG kv

# run cargo test
export RUST_BACKTRACE=1
export RUST_LOG=${RUST_LOG}
export VAULT_ADDR=http://127.0.0.1:${PORT}
# Create a short lived token for the renewal test
export SHORT_LIVED_TOKEN=$(docker exec -i -e VAULT_TOKEN=${VAULT_TOKEN} ${CONTAINER_NAME} \
    vault token create -ttl 120s -renewable -format json -address=http://127.0.0.1:8200 | jq -r .auth.client_token)
[ -n "$VAULT_MOUNT" ] && export VAULT_MOUNT=${VAULT_MOUNT}
# write env file for tests
cat <<EOF > ${ENV_FILE}
VAULT_ADDR=$VAULT_ADDR
VAULT_MOUNT=$VAULT_MOUNT
VAULT_TOKEN=$VAULT_TOKEN
EOF
export ENV_FILE=${ENV_FILE}
cargo test ${RELEASE_FLAG} -- --nocapture

# cleanup
cleanup
