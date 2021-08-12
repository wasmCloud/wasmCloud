# Redis Key Value provider

This capability provider implements the "wasmcloud:keyvalue" capability
contract with a redis back-end.

It is multi-threaded and can handle concurrent requests from multiple actors.

Build with 'make'. Test with 'make test'.

The test program in tests/kv_test.rs has example code for using 
each of this provider's functions.
