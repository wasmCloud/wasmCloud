<img alt='kvredis oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fwasmcloud-ocireferences.cosmonic.app%2Fkvredis' />

# Redis Key Value provider

This capability provider implements the [wasmcloud:keyvalue](https://github.com/wasmCloud/interfaces/tree/main/keyvalue) capability contract with a Redis back-end. It is multi-threaded and can handle concurrent requests from multiple components. Each link definition declared for this provider will result in a single Redis connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

If you want multiple components to share the same keyspace/database then you will need to provide the same Redis URL for multiple link definitions (or utilize start-up configuration as discussed below).

The easiest way to use this provider is to pass `wasmcloud.azurecr.io/kvredis:0.19.0` (or newer, check the badge at the top of this README) as the OCI reference parameter to a wash/lattice control "start provider" command.

```bash
wash ctl start provider wasmcloud.azurecr.io/kvredis:0.19.0
```

For the latest OCI reference URLs for all capability providers, see the root of the [capability-providers](https://github.com/wasmCloud/capability-providers) repository.

## Link Definition Configuration Settings

The following is a list of configuration settings available in the link definition.

| Property | Description                                                                                                                                                                                                       |
| :------- | :---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `URL`    | The connection string URL for the Redis database. Note that all authentication information must also be contained in this URL. The URL _must_ start with the `redis://` scheme. Example: `redis://127.0.0.1:6379` |

## Supplying Startup Configuration

This provider also accepts a default URL as a configuration value on startup. If this value is supplied, then this URL will be used for components linked with no values (you must still link the component to the provider, even if there is no data). URLs defined in link definitions take priority over the default URL.

To supply the startup configuration, you can use `wash` to pass the `config-json` parameter to the start command:

```bash
wash ctl start provider wasmcloud.azurecr.io/kvredis:0.19.0 --config-json /path/to/config.json
```

The JSON expected by the provider is an object with a single `url` field:

```json
{ "url": "redis://127.0.0.1:6379" }
```

Note that this URL, like link definition URLs, must also use the URL scheme `redis://`
