# NATS Capability Provider
This capability provider is an implementation of the `wasmcloud:messaging` contract. It exposes publish, request, and subscribe functionality to components.

## Link Definition Configuration Settings
To configure this provider, use the following link settings in link definitions:

| Property | Description |
| :--- | :--- | 
| `SUBSCRIPTION` | A comma-separated list of subscription topics. If a subscription is a queue subscription, follow the subscription with "\|" and the queue group name. For example, the setting `SUBSCRIPTION=example.actor,example.task\|work_queue` subscribes to the topic `example.actor` and the topic `example.task` in the queue group `work_queue`. |
| `URI` | NATS connection uri. If not specified, the default is `0.0.0.0:4222` |
| `CLIENT_JWT` | Optional JWT auth token. For JWT authentication, both `CLIENT_JWT` and `CLIENT_SEED` must be provided. |
| `CLIENT_SEED` | Private seed for JWT authentication. |
