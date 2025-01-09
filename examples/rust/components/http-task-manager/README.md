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


## Build

You can build the HTTP task manager component with the [WAsmcloud SHell `wash`][wash]:

```console
wash build
```

Note that if you'd like to build with `cargo` you need to specify the `--target` option:

```console
cargo build --target=wasm32-wasip1
```

[wash]: https://wasmcloud.com/docs/cli

## Launch

### Start dependencies

As the application depends on a Postgres instance for persistence, let's start an ephemeral one:

```console
docker run \
    --rm \
    -p 5432:5432 \
    -e POSTGRES_PASSWORD=postgres \
    --name pg \
    postgres:16-alpine
```

### Start the application

Now, we're ready to start our component along with the required providers.

First we start a new wasmCloud host:

```console
wash up
```

> [!NOTE]
> This command won't return, so run open a new terminal to continue running commands

To enable our application we'll start to *connect* to Postgres requires setting up some configuration with `wash`:

```console
wash config put default-postgres \
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


Next, we deploy our application:

```console
wash app deploy ./local.wadm.yaml
```

We can confirm that the application was deployed successfully:

```console
wash app list
```

Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

We'll hit the `/ready` endpoint:

```console
curl localhost:8000/ready
```

You should receive output like the following:

```
{"status":"success"}
```

### Migrating the database

Since this application uses a database, we must migrate the database that uses it. For relative ease of use, the application has been written such that it can migrate it's *own* database (and migrations are written idempotently).

While normally a separate component (or manual DB administrator action) would trigger migrations, we can trigger a migration via HTTP:

```console
curl -X \
    POST -H "Content-Type: application/json; charset=utf8" \
    localhost:8000/admin/v1/db/migrate
```

Regardless of how many times you run the migration, you should receive the output below:

```
{"status":"success"}
```

### Adding a new task

To try out adding a new task we can use `curl`:

```console
curl \
    -X POST \
    -H "Content-Type: application/json; charset=utf8" \
    localhost:8000/api/v1/tasks \
    --data '{"group_id": "test", "task_data": {"one":1}}'
```

### Getting a list of existing tasks

To retrieve all existing tasks:

```console
curl localhost:8000/api/v1/tasks
```

> [!NOTE]
> If you have `jq` installed, this would be a great time to use it!

## Tests

You can run the full E2E test suite:

```console
cargo test
```
