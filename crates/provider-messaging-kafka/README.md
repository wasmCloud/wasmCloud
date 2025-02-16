# Kafka Capability Provider

> [!WARNING]
> ⚠️ **THIS PROVIDER IS CURRENTLY EXPERIMENTAL** ⚠️

This capability provider is an implementation of the `wasmcloud:messaging` contract.

It exposes publish and subscribe functionality to components to operate on Kafka topics when connecting to a Kafka-compatible API. At the time of writing, this provider was tested and works well with [Apache Kafka][kafka] and [Redpanda][redpanda].

[kafka]: https://kafka.apache.org/
[redpanda]: https://redpanda.com/

## Named Config Settings

| Property              | Description                                                                                                                                                                                                                                                                |
|-----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `hosts`               | A comma-separated list of bootstrap server hosts. For example, `HOSTS=127.0.0.1:9092,127.0.0.1:9093`. A single value is accepted as well, and the default value is the Kafka default of `127.0.0.1:9092`. This will be used for both the consumer and producer connections |
| `topic`               | The Kafka topic you wish to consume. Any messages on this topic will be forwarded to this component for processing                                                                                                                                                         |
| `consumer_group`      | Consumer group to use when consuming messages                                                                                                                                                                                                                              |
| `consumer_partitions` | Comma delimited list of partitions to use when subscribing to the topic specified by the link.                                                                                                                                                                             |
| `producer_partitions` | Comma delimited list of partitions to use when handling `publish` calls from components (unrelated to the subscription topic)                                                                                                                                              |
> [!WARNING]
> While `hosts` *can* be provided as named configuration, it *should* be provided as a secret, since
> bootstrap server hosts may be considered or contain sensitive information.
>
> While both named config and secrets are currently allowed, in a future version sensitive fields *must* be supplied via secrets. 

## Secrets

| Property              | Description                                                                                                                                                                                                                                                                |
|-----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `hosts`               | A comma-separated list of bootstrap server hosts. For example, `HOSTS=127.0.0.1:9092,127.0.0.1:9093`. A single value is accepted as well, and the default value is the Kafka default of `127.0.0.1:9092`. This will be used for both the consumer and producer connections |
                                                                                                         |

## Limitations

This capability provider only implements the very basic Kafka functionality of producing to a topic and consuming a topic.

Because of this, advanced Kafka users may find that this is implemented without specific optimizations or options and we welcome any additions to this client.

Additionally, running multiple copies of this provider across different hosts was not tested during development, and it's possible that multiple instances of this provider will cause unexpected behavior like duplicate message delivery.

This provider also hard-codes a return topic (`<topic>.reply`) which is passed along to all actors it invokes.

## Testing

To test this provider, do the following:

### 1. Start Kafka

Start a [Kafka][kafka] instance using [docker][docker] (for example, [`bitnami/kafka`][dockerhub-bitnami/kafka]):

```console
docker run --rm \
    -p 9092:9092 \
    -e KAFKA_CFG_NODE_ID=0 \
    -e KAFKA_CFG_PROCESS_ROLES=controller,broker \
    -e KAFKA_CFG_ADVERTISED_LISTENERS=PLAINTEXT://localhost:9092 \
    -e KAFKA_CFG_LISTENERS=PLAINTEXT://0.0.0.0:9092,CONTROLLER://0.0.0.0:9093 \
    -e KAFKA_CFG_LISTENER_SECURITY_PROTOCOL_MAP=CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT \
    -e KAFKA_CFG_CONTROLLER_QUORUM_VOTERS=0@localhost:9093 \
    -e KAFKA_CFG_CONTROLLER_LISTENER_NAMES=CONTROLLER \
    --name messaging-test-kafka \
    bitnami/kafka:3.6.1
```

After kafka finishes starting up, start a consumer that listens on the topic we're going to be creating later:

```console
docker exec -it \
    messaging-test-kafka \
    kafka-console-consumer.sh \
    --consumer.config /opt/bitnami/kafka/config/consumer.properties \
    --bootstrap-server 127.0.0.1:9092 \
    --topic wasmcloud.echo \
    --from-beginning
```

This command won't return, but will listen continuously for any new published messages on the topic.

[dockerhub-bitnami/kafka]: https://hub.docker.com/r/bitnami/kafka

### 2. Build the provider

You can build this provider with standard Rust tooling:

```console
cargo build
```

To be able to load the provider into a wasmcloud host, we must build a [compressed Provider ARchive (PAR)][wasmcloud-docs-par] with [`wash`][wash]:

```console
wash par create \
    --vendor example \
    --compress \
    --name messaging-kafka \
    --destination provider.par.gz \
    --binary ../target/debug/messaging_kafka
```

You should now have a file named `provider.par.gz` in the current folder.

### 3. Start a new wasmcloud host

Use [`wash`][wash] to start a new [wasmcloud][wasmcloud] host:

```console
wash up
```

### 4. Deploy an architecture declaratively with `wadm`

Using [`wadm`][wadm] we can easily create a declarative deployment, using configuration similar to the [`messaging-kafka-demo.wadm.yaml` WADM manifest](./messaging-kafka-demo.wadm.yaml).

```yaml
---
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: messaging-kafka-demo
  annotations:
    version: v0.0.1
    description: |
      Echo demo in Rust, using the WebAssembly Component Model and WebAssembly Interfaces Types (WIT), along with
      the Kafka messaging provider.
spec:
  components:
    - name: echo
      type: component
      properties:
        # NOTE: make sure to `wash build` the echo messaging example!
        image: file://../../examples/rust/components/echo-messaging/build/echo_messaging_s.wasm
      traits:
        # Govern the spread/scheduling of the component
        - type: spreadscaler
          properties:
            instances: 1
        - type: link
          properties:
            target: nats
            namespace: wasmcloud
            package: messaging
            interfaces: [consumer]
            target_config:
              - name: simple-subscription
                properties:
                  topic: wasmcloud.echo

    # Add a capability provider that implements `wasmcloud:messaging` using NATS
    - name: nats
      type: capability
      properties:
        image: ghcr.io/wasmcloud/messaging-nats:0.25.0
```

Then, we must set up the named config that we're expecting to see (`simple-subscription`):

```console
wash config put simple-subscription topic=wasmcloud.echo
```

To deploy the architecture above to your wasmcloud lattice:

```console
wash app deploy wadm.yaml
```

> [!NOTE]
>
> If you ever need to to remove (and possibly redeploy) your application:
>
> ```console
> wash app delete messaging-kafka-demo v0.0.1
> ```

### 5. Send a message on `<topic>`, see it echoed on `<topic>.reply`

Since the `echo-messaging` component returns any message it receives, and this provider adds a `reply_to` of `<topic>.reply`, we can test that our component is working by sending a message over the kafka topic we created earlier `wasmcloud.echo`, and seeing the messaged surfaced on `wasmcloud.echo.reply`.

To do this, make sure you have a consumer open for the `wasmcloud.echo.reply` topic:

```console
docker exec -it \
    messaging-test-kafka \
    kafka-console-consumer.sh \
    --consumer.config /opt/bitnami/kafka/config/consumer.properties \
    --bootstrap-server 127.0.0.1:9092 \
    --topic wasmcloud.echo.reply \
    --from-beginning
```

Then, you should be able to send a message using the kafka container (note that this command will not return, but will instead produce a prompt):

```console
docker exec -it \
    messaging-test-kafka \
    kafka-console-producer.sh \
    --bootstrap-server 127.0.0.1:9092 \
    --topic wasmcloud.echo
```

Messages you send via the producer will be echoed first in the original consumer (`wasmcloud.echo`) and _also_ echoed in `wasmcloud.echo.reply`, which is the work of the `echo-messaging` component and the default functionality of this provider (supplying a generated `reply_to` topic).

[docker]: https://docs.docker.com
[wash]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/wash
[wadm]: https://github.com/wasmCloud/wadm
