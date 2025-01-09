# Messaging Image Processor Worker

This WebAssembly component listens for jobs (in this case, related to image processing) and performs the requested operations on assets stored in object storage (denoted by the individual job data).

As this component is primarily asynchronous in nature, job senders publish messages to the relevant messaging queue (this depends on *which* `wasmcloud:messaging` interface provider (ex. [`messaging-nats`][provider-messaging-nats], [`messaging-kafka`][provider-messaging-kafka]) is used.

Message that trigger job execution are remarkably simple:

```json
{ "job_id": "99c18a1c-6a84-11ef-a8ae-3cf011fe32f1" }
```

This is possible because this component relies on an interlinked component -- the [http-task-manager][component-http-task-manager] for managing jobs, status, and relevant metadata.

[provider-messaging-nats]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-messaging-nats
[provider-messaging-kafka]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-messaging-kafka
[component-http-task-manager]: https://github.com/wasmCloud/wasmCloud/tree/main/examples/rust/components/http-task-manager

## Architecture

This WebAssembly component requires a few other pieces to function:

- The [wasmCloud HTTP task manager component][component-http-task-manager] for managing task status
- A `wasmcloud:blobstore` provider (ex. [`blobstore-fs`][provider-blobstore-fs], [`blobstore-s3`][provider-blobstore-s3], [`blobstore-azure`][blobstore-azure], or a custom one)
- A `wasmcloud:messaging` provider (ex. [`messaging-nats`][provider-messaging-nats], [`messaging-kafka`][provider-messaging-kafka], or a custom one)

If deploying with the [wasmCloud Application Deployment Manager (`wadm`)][wadm], you can use `wadm.yaml` to deploy these pieces automatically via the [included WADM manifest](./wadm.yaml) (i.e. by running `wash app deploy wadm.yaml`).

By default, the `blobstore-fs` and `messaging-nats` providers are used as they require no extra dependencies (ex. docker containers) to run infrastructure.

This component (the image processor) and the HTTP task manager component communicate via [wRPC][wrpc] (and the [NATS messaging system powering wasmCloud][nats]), so there's no need ot hit the external API, WebAssembly function calls are automatically transformed into low-latency distributed RPC invocations to the HTTP task manager.

[nats]: https://wasmcloud.com/docs/deployment/nats/cluster-config
[wasmcloud-wrpc]: https://wasmcloud.com/docs/reference/glossary#wrpc
[provider-blobstore-fs]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-blobstore-fs
[provider-blobstore-s3]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-blobstore-s3
[provider-blobstore-azure]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-blobstore-azure
[wadm]: https://github.com/wasmCloud/wadm

## Prerequisites

- `cargo` >= 1.81
- [`wash`](https://wasmcloud.com/docs/installation) >=0.32.0

## Build

You can build this component with the [WAsmcloud SHell `wash`][wash]:

```console
wash build
```

Note that if you'd like to build with `cargo` you need to specify the `--target` option:

```console
cargo build --target=wasm32-wasip1
```

[wash]: https://wasmcloud.com/docs/cli

## Launch

First start a new [wasmCloud host][wasmcloud-docs-host]:

```console
wash up
```

> [!NOTE]
> This command won't return, so run open a new terminal to continue running commands

Next, deploy the local version of the application (which uses the WebAssembly binary built by `wash build`):

```console
wash app deploy ./local.wadm.yaml
```

Confirm that the application was deployed successfully:

```console
wash app get
```

[wasmcloud-docs-host]: https://wasmcloud.com/docs/concepts/hosts

## Using this component

Assuming the wasmCloud host has been started and the application is deployed, a few steps must be taken to trigger the worker to process a job.

### Create a job with the HTTP task manager

This component operates primarily on jobs, so it's required to create a job first that can be worked on.

See [the docs for the HTTP task manager][component-http-task-manager] for more information on how it works, the basics will be covered below.

First, make sure the HTTP task manager is migrated:

```console
curl -X POST localhost:8000/admin/v1/db/migrate
```

Then, create a new job:

```console
curl \
    -X POST \
    "localhost:8000/api/v1/tasks/submit \
    --data-binary @- <<EOF
{
  "source": {
    "type": "default-image"
  },
  "destination": {
    "type": "blobstore",
    "path": {
      "bucket": "output",
      "key": "default.grayscaled.jpg"
    }
  },
  "image_format": "image/jpg",
  "operations": [
    {
      "type": "grayscale"
    }
  ]
}
EOF
```

> [!NOTE]
> The task above uses the *default* image for the component, which is [`wasmcloud-logo.jpg`](./wasmcloud-logo.jpg) 

You will receive the Job (task) ID as output to the task submission.

### Send a message notifying the worker of the new job

The worker component expects incoming message to be JSON that are formatted to include the task to be executed.

An example of the JSON payload:

```json
{ "job_id": "..." }
```

> ![NOTE]
> See the `ImageProcessingRequest` struct in [`src/processing.rs`](./src/processing.rs) for the complete schema.

You can send the JSON payload via the [`nats` client binary][nats-client-binary] to trigger the component, on the configured NATS subject (or Kafka Topic):

```
nats publish images.processing '{ "job_id": "......"}'
```

> [!NOTE]
> You may encounter errors, and it is best to monitor the wasmCloud host output for errors reported from the provider or host.


## Tests

You can run the full E2E test suite:

```console
cargo test
```

> [!NOTE]
> Ensure no other wasmCloud hosts are running before running the e2e tests.
