# 🐘 SQLDB Postgres Example

This folder contains a WebAssembly component that makes use of:

- The [`wasmcloud:postgres` WIT contract][contract]
- The [`sqldb-postgres-provider`][provider] Capability Provider

[contract]: ./wit/deps/postgres/provider.wit
[provider]: ../../../../crates/provider-sqldb-postgres

## 📦 Dependencies

- `go` 1.21.1
- `tinygo` 0.30
- [`docker`][docker] for easily running instances of [`postgres`]
- [`wash`][wash] for building and running the components and [wasmCloud][wasmcloud] hosts

[docker]: https://docs.docker.com
[wash]: https://wasmcloud.com/docs/installation

## 👟 Quickstart

As with all other examples, you can get started quickly by using whe [Wasmcloud SHell (`wash`)][wash].

Since `wash` supports declarative deployments (powered by [Wasmcloud Application Deployment Manager (`wadm`)][wadm]), you can get started quickly using the `local.wadm.yaml` manifest in this folder:

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

This will create a folder called `build` which contains `sqldb-postgres-query_s.wasm`.

> [!NOTE]
> If you're using a local build of the provider (using `file://...` in `local.wadm.yaml`) this is a good time to ensure you've built the [provider archive `par.gz`][par] for your provider.

### Start a wasmCloud host

```console
wash up
```

> [!NOTE] > `wash up` will run as long as the host is running (you can cancel it with `Ctrl-C`)

## Set up configuration for the provider

Since configuration for Database clusters is usually sensitive information, we must pre-establish configuration for the provider using the [named configuration feature of wasmCloud][named-config].

### Named configuration setup

Set up a named configuration for components that link to the sqldb-postgres provider which is used from [`local.wadm.yaml`](./local.wadm.yaml):

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
wash app deploy --replace local.wadm.yaml
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
wash call go_sqldb_postgres_query-querier wasmcloud:examples/invoke.call
```

Note that the name of the component is prefixed with the WADM application, and the interface on it we call is defined in `wit/provider.wit` (the `call` function of the `invoke` interface).

## ⌨️ Code guide

With [wasmCloud][wasmcloud], you write only the important bits of your business logic, so the code for this component is short, with the important bits highlighted below:

```go
func (c Component) Call() string {
    query := interfaces.WasmcloudPostgres0_1_0_draft_QueryQuery(CREATE_TABLE_QUERY, make([]PgValue, 0))
    if query.IsErr() {
        return fmt.Sprintf("ERROR: failed to create table: %v", query.UnwrapErr())
    }
    val := interfaces.WasmcloudPostgres0_1_0_draft_TypesPgValueText("inserted example go row!")
    insertResult := interfaces.WasmcloudPostgres0_1_0_draft_QueryQuery(INSERT_QUERY, []PgValue{val})
    if insertResult.IsErr() {
        return fmt.Sprintf("ERROR: failed to insert row: %v", insertResult.UnwrapErr())
    }
    insertedRows := insertResult.Unwrap()
    var rowEntry []RowEntry
    if len(insertedRows) == 1 {
        rowEntry = insertedRows[0]
    } else {
        return "ERROR: failed to insert row"
    }

    for _, row := range rowEntry {
        if row.ColumnName == "description" {
            return fmt.Sprintf("SUCCESS: inserted and retrieved: %v", row.Value.GetText())
        }
    }
    return "ERROR: failed to retrieve inserted row"
}
```

[wasmcloud]: https://wasmcloud.com/docs/intro
