# Redis Key Value provider

This capability provider implements the `wasmcloud:keyvalue` capability contract with a Redis back-end. It is multi-threaded and can handle concurrent requests from multiple actors.

Build with `make`. Test with `make test`.

The test program in tests/kv_test.rs has example code for using
each of this provider's functions.

## Link Definition Configuration Settings

The following is a list of configuration settings available in the link definition.

| Property | Description                                                                                                                   |
| :------- | :---------------------------------------------------------------------------------------------------------------------------- |
| `URL`    | The connection string URL for the Redis database. Note that all authentication information must also be contained in this URL |

## Configuring a default Redis URL

This provider also accepts a default URL as a configuration value on startup to override the default URL. This can be useful to easily setup multiple actors to access the same default endpoint without specifying the URL in the link definition.

```json
{ "url": "redis://127.0.0.1:6380" }
```

```
wash ctl start wasmcloud.azurecr.io/kvredis:0.18.0 --config-json /path/to/config.json
```
