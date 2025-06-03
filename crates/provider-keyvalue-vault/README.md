# Hashicorp Vault capability provider for the wasmcloud KeyValue capability contract wasmcloud:keyvalue

This server uses the [kv v2 secrets engine](https://www.vaultproject.io/docs/secrets/kv/kv-v2), which must be enabled
on the vault before use.

## Link definition configuration settings

The following configuration settings can be set in a link definition or in environment variables.

| Property | Description                                                                                                                                                                                                                 |
|:---------|:----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `token`  | Required. Token for authenticated access. The environment variable `VAULT_TOKEN` overrides this setting.                                                                                                                    |
| `addr`   | Optional url address for connecting to the vault, such as 'https://server:8200'. The environment variable `VAULT_ADDR` overrides this setting. If neither `addr` nor `VAULT_ADDR` are set, `http://127.0.0.1:8200` is used. |
| `mount`  | Optional mount point for keyspace. The environment variable `VAULT_MOUNT` overrides this setting. If neither are specified, `secret/` is used.                                                                              |
| `certs`  | Optional comma-separated list of files containing CA certificates and/or other TLS client certificates to be loaded. Can also be set with the environment variable `VAULT_CACERT`.                                          |

If either `certs` or `VAULT_CACERT` is set, the provider will use TLS to connect to Vault (and the `addr`(VAULT_ADDR) url should begin with `https:`),
otherwise TLS will be disabled (and `addr`(VAULT_ADDR) should begin with `http:`).

For convenience, link setting names may be provided in uppercase or lowercase. Environment variable names are all-caps.
If a setting is provided in the linkdef and in the environment, the environment value takes precedence.

## Supported KeyValue operations

This provider does not support all wasmcloud:keyvalue interface operations.
Unimplemented operations return RpcError::NotImplemented.

Vault stores values as json values.

| Operation       | Result                                                                                                                                                                                                              |
|-----------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Set             | sets secret to the string value. Internally, uses key as the secret path and stores the string value in a hashmap { date: value }. Can return error if user does not have permission to write to the key path.      |
| Get             | gets the string value of a secret key. Loads hashmap from key path and returns the data field of the wrapping hashmap. Returns error if the key does not exist or the user does not have access to read the secret. |
| Contains        | returns true if there is a secret at the key path and it is readable.                                                                                                                                               |
| Del             | deletes the latest version of the key.                                                                                                                                                                              |
| SetQuery        | returns the list of secret keys in the requested path.                                                                                                                                                              |
| Increment       | unsupported                                                                                                                                                                                                         |
| ListAdd         | unsupported                                                                                                                                                                                                         |
| ListClear       | unsupported                                                                                                                                                                                                         |
| ListDel         | unsupported                                                                                                                                                                                                         |
| ListRange       | unsupported                                                                                                                                                                                                         |
| SetAdd          | unsupported                                                                                                                                                                                                         |
| SetDel          | unsupported                                                                                                                                                                                                         |
| SetIntersection | unsupported                                                                                                                                                                                                         |
| SetUnion        | unsupported                                                                                                                                                                                                         |
