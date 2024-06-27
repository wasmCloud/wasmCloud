# Secrets

Support for application secrets was added to wasmCloud 1.1.0 after the community accepted [RFC #2190](https://github.com/wasmCloud/wasmCloud/issues/2190).

This folder has an example of an application that has a capability provider and component that refer to a secret value. The secret itself is encrypted and stored in a [NATS KV secrets backend](../../../crates/secrets-nats-kv/) instance, and wasmCloud fetches this secret at runtime based on the signed identity of the component/provider.

**This is a modified version of the [http-keyvalue-counter](../../rust/components/http-keyvalue-counter/) application that authenticates with a Redis database that requires a password, and serves an HTTP API that requires an authentication password header.**

This example uses a locally built version of the NATS KV secrets backend and wasmCloud from the `main` branch of this repository, as wasmCloud 1.1 is not yet released. Additionally, support for secrets in the [wadm](https://github.com/wasmCloud/wadm) application manifest was not released at the time of writing this example, so the application and setup is done imperatively. We'll update this example to also have an application manifest for easy setup once available.

## Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install)
- [wash](https://wasmcloud.com/docs/installation)
- [NATS CLI](https://github.com/nats-io/natscli)
- [redis-server](https://redis.io/docs/latest/operate/oss_and_stack/install/install-redis/)
- [nats-server](https://docs.nats.io/running-a-nats-service/introduction)
- [jq](https://jqlang.github.io/jq/download/)

## Running this example

Build wasmCloud, the NATS KV secrets backend, the keyvalue counter auth component, and the keyvalue redis password provider:

```bash
./build.sh
```

Run the example:

```bash
./run.sh
```

Once running, in a different terminal you can first verify that unauthenticated requests to Redis and the component are denied:

```bash
➜ redis-cli -u redis://127.0.0.1:6379 keys '*'
(error) NOAUTH Authentication required.

➜ curl 127.0.0.1:8080/counter
Unauthorized
```

Then, authenticating passes the check in the component:

```bash
➜ curl -H "password: opensesame" 127.0.0.1:8080/counter
Counter /counter: 1
```

Passing in an invalid password will still fail the authentication check:

```bash
➜ curl -H "password: letmein" 127.0.0.1:8080/counter
Unauthorized
```

If you want to inspect the Redis database directly, you can provide the password in the URI:

```bash
redis-cli -u redis://sup3rS3cr3tP4ssw0rd@127.0.0.1:6379 get /counter
```
