# source this file to set the vault token in your current environment

CONTAINER_NAME=kv-vault-test
export VAULT_TOKEN="$(docker logs ${CONTAINER_NAME} 2>&1 | grep 'Root Token:' | sed -E 's/Root Token: //')"

