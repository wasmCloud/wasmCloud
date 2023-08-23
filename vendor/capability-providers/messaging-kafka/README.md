⚠️ **THIS PROVIDER IS CURRENTLY EXPERIMENTAL** ⚠️

# Kafka Capability Provider

This capability provider is an implementation of the `wasmcloud:messaging` contract. It exposes publish and subscribe functionality to actors to operate on Kafka topics when connecting to a Kafka-compatible API. At the time of writing, this provider was tested and works well with Apache Kafka and Redpanda.

## Link Definition Configuration Settings

| Property | Description                                                                                                                                                                                                                                                                |
| :------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `HOSTS`  | A comma-separated list of bootstrap server hosts. For example, `HOSTS=127.0.0.1:9092,127.0.0.1:9093`. A single value is accepted as well, and the default value is the Kafka default of `127.0.0.1:9092`. This will be used for both the consumer and producer connections |
| `TOPIC`  | The Kafka topic you wish to consume. Any messages on this topic will be forwarded to this actor for processing |

## Limitations

This capability provider only implements the very basic Kafka functionality of producing to a topic and consuming a topic. Because of this, advanced Kafka users may find that this is implemented without specific optimizations or options and we welcome any additions to this client. Additionally, running multiple copies of this provider across different hosts was not tested during development, and it's possible that multiple instances of this provider will cause unexpected behavior like duplicate message delivery.