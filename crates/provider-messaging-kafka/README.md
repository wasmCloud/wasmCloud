# Kafka Capability Provider

> [!WARNING]
> ⚠️ **THIS PROVIDER IS CURRENTLY EXPERIMENTAL** ⚠️

This capability provider is an implementation of the `wasmcloud:messaging` contract.

It exposes publish and subscribe functionality to components to operate on Kafka topics when connecting to a Kafka-compatible API. At the time of writing, this provider was tested and works well with [Apache Kafka][kafka] and [Redpanda][redpanda].

[kafka]: https://kafka.apache.org/
[redpanda]: https://redpanda.com/

## Link Definition Configuration Settings

| Property | Description                                                                                                                                                                                                                                                                |
|:---------|:---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `HOSTS`  | A comma-separated list of bootstrap server hosts. For example, `HOSTS=127.0.0.1:9092,127.0.0.1:9093`. A single value is accepted as well, and the default value is the Kafka default of `127.0.0.1:9092`. This will be used for both the consumer and producer connections |
| `TOPIC`  | The Kafka topic you wish to consume. Any messages on this topic will be forwarded to this component for processing                                                                                                                                                             |

## Limitations

This capability provider only implements the very basic Kafka functionality of producing to a topic and consuming a topic.

Because of this, advanced Kafka users may find that this is implemented without specific optimizations or options and we welcome any additions to this client.

Additionally, running multiple copies of this provider across different hosts was not tested during development, and it's possible that multiple instances of this provider will cause unexpected behavior like duplicate message delivery.

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
    --topic messaging-kafka.test \
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
    --capid wasmcloud:messaging \
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

Using [`wadm`][wadm] we can easily create a declarative deployment (see [`messaging-kafka-test.wadm.yaml`](./messaging-kafka-test.wadm.yaml)):

```yaml
---
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: messaging-kafka-test
  annotations:
    version: v0.0.1
    description: "Test messaging-kafka provider with test actor messaging-sender-http-smithy"
    experimental: true
spec:
  components:
    # (Capability Provider) mediates HTTP access
    - name: httpserver
      type: capability
      properties:
        image: wasmcloud.azurecr.io/httpserver:0.19.1
        contract: wasmcloud:httpserver

    # (Capability Provider) provides messaging with Kafka
    - name: messaging-kafka
      type: capability
      properties:
        # TODO: you must replace the path below with the provider par.gz generated earlier
        image: file:///the/absolute/path/to/provider.par.gz
        contract: wasmcloud:messaging

    # (Actor) A test actor that turns HTTP requests into Kafka messages
    # in particular, sending a HTTP POST request to `/publish` will trigger a publish
    - name: messaging-receiver-http-smithy
      type: actor
      properties:
        # TODO: you must replace the path below to match your genreated code in build
       image: file:///the/absolute/path/to/build/messaging-sender-http-smithy_s.wasm
      traits:
        # Govern the spread/scheduling of the actor
        - type: spreadscaler
          properties:
            replicas: 1

        # Link the HTTP server, and inform it to listen on port 8081
        # on the local machine
        - type: linkdef
          properties:
            target: httpserver
            values:
              ADDRESS: 127.0.0.1:8081

        # Link to the messaging provider, directing it to the Kafka host
        # and topics to listen on/interact with for this actor
        - type: linkdef
          properties:
            target: messaging
            values:
              HOSTS: 127.0.0.1:9092
              TOPIC: messaging-kafka.test
```

To deploy the architecture above to your wasmcloud lattice:

```console
wash app deploy messaging-kafka-test.wadm.yaml
```

> [!NOTE]
>
> If you ever need to to remove (and possibly redeploy) your application:
>
> ```console
> wash app delete messaging-kafka-test v0.0.1
> ```

### 5. Send a HTTP request to trigger a publish

Since we're using the [`messaging-sender-http-smithy`][project-messaging-sender-http-smithy] actor, we can use a HTTP request to trigger a mesasge publish on our `messaging-kafka.test` topic.

```console
curl localhost:8081/publish \
--data-binary @- << EOF
{
  "msg": {
    "subject": "messaging-kafka.test",
    "body": "hello world"
  }
}
EOF
```

> [!WARNING]
> The first few calls may fail with a 500 until the Kafka connection is established on the provider side!

Once your `curl` call is successful, you should see output:

```
$ curl localhost:8081/publish --data @publish.json
{"data":null,"status":"success"}
```

In the shell where you started the consumer, you should also see "hello world" printed to the screen.

[docker]: https://docs.docker.com
[wash]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/wash-cli
[wadm]: https://github.com/wasmCloud/wadm
