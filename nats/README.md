# Nats capability provider for wasmcloud:messaging

The nats capability provider exposes publish and subscribe functionality to actors. To configure, use the following link settings:

- `SUBSCRIPTION` a comma-separated list of subscription topics. If a subscription is a queue subscription, follow the subscription with `|` and the queue group name. For example, the setting
    `SUBSCRIPTION=example.actor,example.task|work_queue`
subscribes to the topic "example.actor" and the topic "example.task" in the queue group "work_queue".
- `URI` nats connection uri. If not specified, the default is '0.0.0.0:4222'
- `CLIENT_JWT` jwt auth token. For jwt authentication, both CLIENT_JWT and CLIENT_SEED must be provided.
- `CLIENT_SEED` private seed for jwt authentication.

For examples of invoking the messaging api, see
- The example actor [nats-messaging](https://github.com/wasmCloud/examples/tree/main/actor/nats-messaging)
