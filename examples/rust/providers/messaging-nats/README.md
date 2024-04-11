# NATS Capability Provider

This capability provider is an implementation of the `wasmcloud:messaging` contract. It exposes publish, request, and subscribe functionality to components.

## Link Configuration

To configure this provider, use the following configuration values as `target_config` in the link:

| Property        | Description                                                                                                                                                                      |
| :-------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `subscriptions` | A comma-separated list of subscription topics. For example, the setting `subscription=wasmcloud.example,test.topic` subscribes to the topic `wasmcloud.example` and `test.topic` |
| `uri`           | NATS connection URI. If not specified, the default is `0.0.0.0:4222`                                                                                                             |

## Full Implementation

This provider is fully implemented as [provider-messaging-nats](https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-messaging-nats) in the wasmCloud repository. You can find the full implementations for the functions marked as `TODO:` in that crate, as well as extensions that add NATS authentication, TLS CA support, etc.
