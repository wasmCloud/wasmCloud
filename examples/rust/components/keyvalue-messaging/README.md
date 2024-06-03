# NATS KeyValue Messaging

This is a Wasm component example, which using NATS demonstrates the following operations:

*   Increments the value of a `key`, *one or more times*, in an existing `bucket`.&#x20;

    *   The ***key*** name is received by ***subscribing*** to a NATS subject, which its name is passed as configuration data.

    *   The ***bucket name*** and ***repetition factor*** are passed as component configuration data.

*   Reads the the above key-value pair, from the bucket, and publishes the results to a NATS `subject`, which the name is also passed as component configuration data.

*   Lists all keys in the bucket, and publishes them to the same NATS subject.

*   Deletes the key-value pair from the bucket. This is the equivalent of issuing the NATS cli command `nats kv purge <bucket-name> <key-name>`.

*   All results of operations are logged and can be viewed in the *\~/.wash/downloads/wascloud.log*.

It worth noting that this example, either directly or indirectly, tests all the core *NATS Kv Store* functions, underpining implementation of the *wasi:keyvalue* interfaces' functions (except for watch).

## Prerequisites

*   `cargo` 1.78

*   [`wash`](https://wasmcloud.com/docs/installation) 0.28.1

*   `nats cli` 0.1.4

## Building

From the root directory of this example issue the following command:

```bash
wash build
```

## Running with wasmCloud

The NATS bucket used must exist, before running the example; so, run the following cli commands to ensure wasmCloud is running, and we have a new bucket, which we can watch:

```shell
# Start wasmCloud
wash up --detach

# Add a new NATS KV bucket
nats kv add WASMCLOUD

# Deply the example
wash app deploy ./wadm.yaml
wash app list

# Watch the newly created NATS bucket for changes:
nats kv watch WASMCLOUD

# In a different terminal window, subscribe to the topic, which the example component would publish retrieved key-value pair:
nats sub 'nats.atomic'

# In another terminal window, trigger the component's operations, by publishing the name of the key, which must be incremented:
nats pub 'nats.keys' counter
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
