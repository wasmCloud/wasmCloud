# HTTP Task manager

This web component tracks and updates information about asynchronous tasks, backed by a [Postgres][postgres] Database.

Clients can make HTTP requests to endpoints like the following:

| Endpoint                 | Description                   |
|--------------------------|-------------------------------|
| `GET /tasks`             | List ongoing tasks            |
| `POST /tasks/:id/status` | Get status of a specific task |
| `POST /tasks`            | Submit a new task             |

Tasks can also be submitted over the wasmCloud Lattice (i.e. [via wRPC][wasmcloud-wrpc]), rather than simply via HTTP.

## Architecture

This application requires a few other pieces to function:

- [wasmCloud HTTP server][provider-http-server] for receiving incoming HTTP requests
- [wasmCloud SQLDB Postgres Provider][provider-sqldb-postgres] for persistence of tasks

[postgres]: https://www.postgresql.org/
[wasmcloud-wrpc]: https://wasmcloud.com/docs/reference/glossary#wrpc
[provider-http-server]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-http-server
[provider-sqldb-postgres]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-sqldb-postgres

## Prerequisites

- `cargo` >= 1.81
- [`wash`](https://wasmcloud.com/docs/installation) >=0.30.0
- `docker` (or some other means to easily run Postgres instances)

## Building

You can build the application with the [WAsmcloud SHell `wash`][wash]:

```console
wash build
```

[wash]: https://wasmcloud.com/docs/cli

## Start dependencies

As the application depends on a Postgres instance for persistence, let's start an ephemeral one:

```
docker run \
    --rm \
    -p 5432:5432 \
    -E POSTGRES_PASSWORD=postgres \
    postgres:16-alpine
```

Enabling the application to *connect* to Postgres requires setting up some secrets, which we can set with `wash`:

```console
wash config put pg-task-db \
    POSTGRES_HOST=localhost \
    POSTGRES_PORT=5432 \
    POSTGRES_USERNAME=postgres \
    POSTGRES_PASSWORD=postgres \
    POSTGRES_DATABASE=postgres \
    POSTGRES_TLS_REQUIRED=false
```

> [!NOTE]
> In production, you'll want to use wasmCloud secrets to set up the required configuration.
>
> See [wasmCloud documentation for secrets][wasmcloud-secrets]

[wasmcloud-secrets]: https://wasmcloud.com/docs/concepts/secrets

## Starting the application

Now, we're ready to start our component along with the required providers.

First we start a new wasmCloud host:

```console
wash up
```

> [!NOTE]
> This command won't return, so run open a new terminal to continue running commands

Next, we deploy our application:

```console
wash app deploy ./local.wadm.yaml
```

We can confirm that the application was deployed successfully:

```console
wash app list
```

Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```console
curl http://127.0.0.1:8080
```
