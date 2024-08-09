# 🐘 SQL Database Postgres Provider

This capability provider implements the [`wasmcloud:sqldb-postgres`][wasmcloud-sqldb-postgres-wit] WIT package, which enables SQL-driven database interaction with a [Postgres][postgres] database cluster.

This provider handles concurrent component connections, and components which are linked to it should specify configuration at link time (see [the named configuration settings section](#named-configuration-settings) for more details.

Want to read all the functionality included the interface? [Start from `provider.wit`](./wit/provider.wit) to read what this provider can do, and work your way to [`types.wit`](./wit/types.wit).

Note that connections are local to a single provider, so multiple providers running on the same lattice will _not_ share connections automatically.

[postgres]: https://postgresql.org
[wasmcloud-sqldb-postgres-wit]: https://github.com/vados-cosmonic/wit-wasmcloud-postgres

## 👟 Quickstart

To get this provider started quickly, you can start with:

```console
wash start provider ghcr.io/wasmcloud/provider-sqldb-postgres:0.5.2
```

The easiest way to start a Postgres provider with configuration specified, and a component that uses it is with [wasmCloud Application Deployment Manager][wadm].

<details>
<summary>Example manifest for an HTTP server with a database connection</summary>

```yaml
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: sqldb-postgres-example
  annotations:
    version: v0.0.1
    description: SQLDB Postgres example
spec:
  components:
    # A capability provider that enables Postgres access for the component
    - name: sqldb-postgres
      type: capability
      properties:
        image: ghcr.io/wasmcloud/sqldb-postgres:0.5.2

    # A capability provider that provides HTTP serving for the component
    - name: http-server
      type: capability
      properties:
        image: ghcr.io/wasmcloud/http-server:0.22.0

    # A component that uses both capability providers above (HTTP server and sqldb-postgres)
    # to provide a TODO app on http://localhost:8080
    - name: todo-app
      type: component
      properties:
        image: ghcr.io/wasmcloud/component-todoapp-postgres-rust:0.1.0
      traits:
        # Govern the spread/scheduling of the component
        - type: spreadscaler
          properties:
            replicas: 1

        # Link the httpserver to the component, and configure the HTTP server
        # to listen on port 8080 for incoming requests
        - type: link
          properties:
            target: http-server
            namespace: wasi
            package: http
            interfaces: [incoming-handler]
            source_config:
              - name: default-http
                properties:
                  address: 127.0.0.1:8080

        # Link the sqldb-provider to the component, specifying the postgres cluster URL
        - type: link
          properties:
            target: sqldb-postgres
            namespace: wasmcloud
            package: sqldb-postgres
            interfaces: [query, prepared]
            # NOTE: When configuraiton is specified below only by name, it references a named configuration
            # (ex. one set via `wash config put`)
            target_config:
              - name: pg
```

</details>

[wadm]: https://github.com/wasmCloud/wadm

## 📑 Named configuration Settings

As connection details are considered sensitive information, they should be specified via named configuration to the provider, and _specified_ via link definitions.
WADM files should not be checked into source control containing secrets.

New named configuration can be specified by using `wash config put`.

| Property                | Example     | Description                                               |
| ----------------------- | ----------- | --------------------------------------------------------- |
| `POSTGRES_HOST`         | `localhost` | Postgres cluster hostname                                 |
| `POSTGRES_PORT`         | `5432`      | Postgres cluster port                                     |
| `POSTGRES_USERNAME`     | `postgres`  | Postgres cluster username                                 |
| `POSTGRES_TLS_REQUIRED` | `false`     | Whether TLS should be required for al managed connections |

Once named configuration with the keys above is created, it can be referenced as `target_config` for a link to this provider.

For example, the following WADM manifest fragment:

```yaml
- name: querier
  type: component
  properties:
    image: file://./build/sqldb_postgres_query_s.wasm
  traits:
    - type: spreadscaler
      properties:
        replicas: 1
    - type: link
      properties:
        target: sqldb-postgres
        namespace: wasmcloud
        package: postgres
        interfaces: [query]
        target_config:
          - name: default-postgres
```

The `querier` component in the snippet above specifies a link to a `sqldb-postgres` target, with `target_config` that is only specifies `name` (no `properties`).

> [!WARNING]
> While `POSTGRES_PASSWORD` can be specified as named configuration, it should be specified as a secret.
>
> In a future version, this will be required.

## 🔐 Secret Settings

While most values can be specified via named configuration, sensitive values like the `POSTGRES_PASSWORD` should be speicified via *secrets*.

New secrets be specified by using `wash secrets put`.

| Property                | Example     | Description                                               |
| ----------------------- | ----------- | --------------------------------------------------------- |
| `POSTGRES_PASSWORD`     | `postgres`  | Postgres cluster password                                 |

Once a secret has been created, it can be referenced in the link to the provider.

For example, the following WADM manifest fragment:

```yaml
- name: querier
  type: component
  properties:
    image: file://./build/sqldb_postgres_query_s.wasm
  traits:
    - type: spreadscaler
      properties:
        replicas: 1
    - type: link
      properties:
        target: sqldb-postgres
        namespace: wasmcloud
        package: postgres
        interfaces: [query]
        target_secrets:
          - name: default-postgres-secrets
```

The `querier` component in the snippet above specifies a link to a `sqldb-postgres` target, with `target_config` that is only specifies `name` (no `properties`).

## 📦 Building a PAR

To build a [Provider Archive (`.par`/`.par.gz`)][par] for this provider, first build the project with `wash`:

```console
wash build
```

Then run `wash par`:

```
wash par create \
  --compress
  --binary target/debug/sqldb-postgres-provider
  --vendor wasmcloud
  --version 0.1.0
  --name sqldb-postgres-provider`
```

[par]: https://wasmcloud.com/docs/developer/providers/build
