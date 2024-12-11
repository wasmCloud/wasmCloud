# Secrets

Support for application secrets was added to wasmCloud 1.1.0 after the community accepted [RFC #2190](https://github.com/wasmCloud/wasmCloud/issues/2190).

This folder has an example of an application that has a capability provider and component that refer to a secret value. The secret itself is encrypted and stored in a [NATS KV secrets backend](../../../crates/secrets-nats-kv/) instance, and wasmCloud fetches this secret at runtime based on the signed identity of the component/provider.

**This is a modified version of the [http-keyvalue-counter](../../rust/components/http-keyvalue-counter/) application that authenticates with a Redis database that requires a password, and serves an HTTP API that requires an authentication password header.**

## Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install)
- [wash 0.30.0 or newer](https://wasmcloud.com/docs/installation)
- [Docker](https://www.docker.com/)
- [jq](https://jqlang.github.io/jq/download/)
- [secrets-nats-kv](../../../crates/secrets-nats-kv/) installed

## Running this example

Build the keyvalue counter auth component, and the keyvalue redis auth provider:

```bash
wash build -p component-keyvalue-counter-auth
wash build -p provider-keyvalue-redis-auth
```

Run the example docker compose for the necessary infrastructure, generating secret keys:

```bash
export ENCRYPTION_XKEY_SEED=$(wash keys gen curve -o json | jq -r '.seed')
export TRANSIT_XKEY_SEED=$(wash keys gen curve -o json | jq -r '.seed')
docker compose up -d
```

Place the necessary secrets in the NATS KV backend:

```bash
# Ensure the TRANSIT_XKEY_SEED is still exported in your environment above
# or the decryption of the secret will fail
secrets-nats-kv put api_password --string opensesame
secrets-nats-kv put redis_password --string sup3rS3cr3tP4ssw0rd
# You can also put the password using an environment variable
SECRET_STRING_VALUE=sup3rS3cr3tP4ssw0rd secrets-nats-kv put default_redis_password
```

Allow your component and provider to access these secrets at runtime (this is a NATS KV backend specific step, other secrets backends like Vault will handle authorization externally with policies):

```bash
component_key=$(wash inspect ./component-keyvalue-counter-auth/build/component_keyvalue_counter_auth_s.wasm -o json | jq -r '.component')
provider_key=$(wash inspect ./provider-keyvalue-redis-auth/build/wasmcloud-example-auth-kvredis.par.gz -o json | jq -r '.service')
secrets-nats-kv add-mapping $component_key --secret api_password
secrets-nats-kv add-mapping $provider_key --secret redis_password --secret default_redis_password
```

Lastly, run wasmCloud and deploy the application:

```bash
WASMCLOUD_SECRETS_TOPIC=wasmcloud.secrets \
    WASMCLOUD_ALLOW_FILE_LOAD=true \
    NATS_CONNECT_ONLY=true \
    wash up --detached
```

```bash
wash app deploy ./wadm.yaml
```

You can check the status of your application by running `wash app get`. Once it's deployed, you can make requests to the application.

## Making authenticated requests

You can first verify that unauthenticated requests to Redis and the component are denied:

```bash
➜ redis-cli -u redis://127.0.0.1:6379 keys '*'
(error) NOAUTH Authentication required.

➜ curl 127.0.0.1:8000/counter
Unauthorized
```

Then, authenticating passes the check in the component:

```bash
➜ curl -H "password: opensesame" 127.0.0.1:8000/counter
Counter /counter: 1
```

Passing in an invalid password will still fail the authentication check:

```bash
➜ curl -H "password: letmein" 127.0.0.1:8000/counter
Unauthorized
```

If you want to inspect the Redis database directly, you can provide the password in the URI:

```bash
redis-cli -u redis://sup3rS3cr3tP4ssw0rd@127.0.0.1:6379 get /counter
```

## Cleanup

When finished with this example, simply shutdown wasmCloud and the resources running in Docker:

```bash
wash down
docker compose down
```
