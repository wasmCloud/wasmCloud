# NATS KeyValue Messaging

This is a Wasm component example, which, using NATS KeyValue and Messaging providers, demonstrates the following operations:

*   Performs a variety of *wasi:keyvalue* *store* and *atomics* interfaces operations, using either one or two NATS `buckets`.&#x20;

    *   The ***bucket names*** are passed as provider configuration data (i.e. under ***target\_config*** attribute), via the component ***named*** link definitions.

        *   The example can work with either *one*, or *two* link definitions, demonstrating how to provide multiple configuration sets to the provider, and switch between them.

    *   The ***link names***, which identify the *target buckets* are passed as component configuration data. They're used to reference the target buckets for keyvalue operations.

    *   For the *increment* function, the ***delta*** value is received by ***subscribing*** to a NATS ***subject***, which its name is passed as part of *wasmcloud-messaging* provider configuration data (under ***source\_config*** attribute).

*   The results of operations are published to another NATS `subject`, which the name is also passed as component configuration data.

*   The results are also logged and can be viewed in the *\~/.wash/downloads/wascloud.log*.

> The *delete* operation is the equivalent of issuing the NATS cli command `nats kv purge <bucket-name> <key-name>`.

## Prerequisites

*   `cargo` 1.78

*   [`wash`](https://wasmcloud.com/docs/installation) 0.29.2

*   `wadm` 0.12.2 (only if multiple links used)

*   `nats cli` 0.1.4

## Building

From the root directory of this example issue the following command:

```bash
wash build
```

## Running with wasmCloud

The NATS buckets used must exist, before running the example; so, run the following cli commands to ensure wasmCloud is running, and we have new buckets, which we can watch:

```shell
# Start wasmCloud
wash up --detach

# Add 2 new NATS KV buckets
nats kv add WASMCLOUD
nats kv add WASMLAND

# Deply the example
wash app deploy ./wadm.yaml
wash app get

# Watch the newly created NATS buckets for changes:
nats kv watch WASMCLOUD &
nats kv watch WASMLAND &

# In a different terminal window, subscribe to the topic, which the example component would publish retrieved key-value pair:
nats sub 'nats.demo'

# In another terminal window, trigger the component's operations, by publishing the delta of the counter key:
nats pub 'nats.atomic.delta' 100
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
