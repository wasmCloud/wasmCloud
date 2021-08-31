# Redis Key Value provider

This capability provider implements the `wasmcloud:keyvalue` capability contract with a Redis back-end. It is multi-threaded and can handle concurrent requests from multiple actors.

Build with `make`. Test with `make test`.

The test program in tests/kv_test.rs has example code for using 
each of this provider's functions.

## Link Definition Configuration Settings
The following is a list of configuration settings available in the link definition.

| Property | Description |
| :--- | :--- |
| `URL` | The connection string URL for the Redis database. Note that all authentication information must also be contained in this URL |
