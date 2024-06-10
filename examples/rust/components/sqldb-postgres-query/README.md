# 🐘 SQLDB Postgres Example

This folder contains a WebAssembly component that makes use of:

- The [`wasmcloud:postgres` WIT contract][contract]
- The [`sqldb-postgres-provider`][provider] Capability Provider

[contract]: ./wit/deps/postgres/provider.wit
[provider]: ../../../../crates/provider-sqldb-postgres

## 📦 Dependencies

- [`docker`][docker] for easily running instances of [`postgres`]
- [`cargo`][cargo] (part of the Rust toolchain) for building this project
- [`wash`][wash] for building and running the components and [wasmCloud][wasmcloud] hosts

[docker]: https://docs.docker.com
[postgres]: https://postgresql.org
[cargo]: https://doc.rust-lang.org/cargo/

## 👟 Quickstart

As with all other examples, you can get started quickly by using whe [Wasmcloud SHell (`wash`)][wash].

Since `wash` supports declarative deployments (powered by [Wasmcloud Application Deployment Manager (`wadm`)][wadm]), you can get started quickly using the `wadm.yaml` manifest in this folder:

## Start a local Postgres cluster

Before we can connect to a Postgres database cluster, we'll need to have one running. You can run one quickly with `docker`:

```console
docker run \
    --rm \
    -e POSTGRES_PASSWORD=postgres \
    --name pg -p 5432:5432\
    postgres:16.2-alpine
```

### Build this component

```console
wash build
```

This will create a folder called `build` which contains `sqldb_postgres_query_s.wasm`.

> [!NOTE]
> If you're using a local build of the provider (using `file://...` in `wadm.yaml`) this is a good time to ensure you've built the [provider archive `par.gz`][par] for your provider.

### Start a wasmCloud host

```console
wash up
```

> [!NOTE]
> `wash up` will run as long as the host is running (you can cancel it with `Ctrl-C`)


## Set up configuration for the provider

Since configuration for Database clusters is usually sensitive information, we must pre-establish configuration for the provider using the [named configuration feature of wasmCloud][named-config].

### Named configuration setup

Set up a named configuration for components that link to the sqldb-postgres provider which is used from [`wadm.yaml`](./wadm.yaml):

```console
wash config put default-postgres \
    POSTGRES_HOST=localhost \
    POSTGRES_PORT=5432 \
    POSTGRES_USERNAME=postgres \
    POSTGRES_PASSWORD=postgres \
    POSTGRES_DATABASE=postgres \
    POSTGRES_TLS_REQUIRED=false
```

### Deploy the example application with WADM

```console
wash app deploy --replace wadm.yaml
```

> [!WARNING]
> If you simply want to stop the deployment, run `wash app delete <application name>`.
>
> In this case, `wash app delete rust-sqldb-postgres` should work.

To ensure that the application is deployed you can use `wadm app list`:

```console
wadm app list
```

If you want to see everything running in the lattice at once:

```console
wash get inventory
```

[wadm]: https://github.com/wasmCloud/wadm

### Invoke the demo component

Once the component & provider are deployed, you can invoke the example component with `wash call`:

```console
wash call rust_sqldb_postgres_query-querier wasmcloud:examples/invoke.call
```

Note that the name of the component is prefixed with the WADM application, and the interface on it we call is defined in `wit/provider.wit` (the `call` function of the `invoke` interface).

## ⌨️ Code guide

With [wasmCloud][wasmcloud], you write only the important bits of your business logic, so the code for this component is short, with the important bits highlighted below:

```rust
impl Guest for QueryRunner {
    fn call() -> String {
        if let Err(e) = query(CREATE_TABLE_QUERY, &[]) {
            return format!("ERROR - failed to create table: {e}");
        };

        match query(
            INSERT_QUERY,
            &[PgValue::Text(format!("inserted example row!"))],
        ) {
            Ok(rows) => format!("SUCCESS: inserted new row:\n{rows:#?}"),
            Err(e) => format!("ERROR: failed to insert row: {e}"),
        }
    }
}
```

[wasmcloud]: https://wasmcloud.com/docs/intro
