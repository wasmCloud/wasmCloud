# sqldb-postgres capability provider

This capability provider allows wasmCloud components to use a Postgres-compatible
database, and implements the "wasmcloud:sqldb" capability contract.

### Capabilities:
- execute statements (create table, insert, update, etc.)
- select statements
- configurable connection pool with sensible defaults
 
### JSON Configuration settings

| Setting                  | Description                                                                                                                                                                                                      |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `uri`                    | database connection string. Must begin with scheme `postgres://` or `postgresql://`. Example:  `postgresql://user:password@host:5678?dbname=customers`. See [uri reference](https://docs.rs/tokio-postgres/0.7.2/tokio_postgres/config/struct.Config.html) for complete documentation on all the options |
| `pool.max_connections`   | max size of connection pool. Default is 8                                                                                                                                                                        |
| `pool.min_idle`          | minimum number of idle connections in pool. Default is 0. With this default, the provider does not consume resources until needed. If you need fast application startup time, you may wish to set this to 1 or more, and increase max_lifetime_secs to 86400.         |
| `pool.max_lifetime_secs` | when a connection has reached this age, after it has finished processing its current workload, it is closed instead of being returned to the pool. Default is 7200 (2 hours).                                    |
| `pool.idle_timeout_secs` | the amount of time a connection will remain idle in the pool before it is closed. This setting can be useful to reduce billing costs if your database is billed by connection-time. Default is 600 (10 minutes). |

### Link

- Edit `linkdefs.json` to adjust your settings. To make these active when linking a component to this provider,
set a link value with key `config_b64` with value from the output of the command
```shell
base64 -w0 linkdefs.json
```

### Limitations:

The following features are not currently supported:
- TLS connections
- transactions
- batch operations
- streaming results
- prepared statements
- query results contain NULL, or any Array type, Custom data type, or other column
type not listed in the table below.


### Supported Postgres data types

Conversion from Postgres data type to CBOR data types

| Supported Data Types | CBOR type     | Notes                                 |
| ---------- | ------------- | --------------------------------------- |
| BOOL       | boolean       |                                         |
| CHAR       | i16 (2 bytes) |                                         |
| INT2       | i16           |                                         |
| INT4       | i32           |                                         |
| INT8       | i64           |                                         |
| OID        | u32           |                                         |
| FLOAT4     | f32           |                                         |
| FLOAT8     | f64           |                                         |
| CHAR_ARRAY | string        |                                         |
| VARCHAR    | string        |                                         |
| TEXT       | string        |                                         |
| NAME       | string        |                                         |
| UNKNOWN    | string        |                                         |
| JSON       | string        |                                         |
| XID        | string        |                                         |
| CID        | string        |                                         |
| XML        | string        |                                         |
| BYTEA      | bytes         |                                         |
| UUID       | string        | uuid converted to string                |
| TIMESTAMP  | string        | RFC3339 format, in UTC                  |
| DATE       | string        |                                         |
| TIME       | string        |                                         |
| INET       | string        | ip address converted to string          |
| BIT        | bytes         | bit vectors converted to byte array     |
| *          | bytes         | All other types returned as raw byte array |


Note that NULL values are not currently supported.
If any query results contain null values, the results 
are undefined and will likely generate errors. If you are querying
tables that contain null values, modify
the SELECT query to replace NULLs with another
value of the same data type as the column.


Build with 'make'. Test with 'make test'.

