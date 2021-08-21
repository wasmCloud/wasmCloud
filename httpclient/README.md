# HTTP client capability provider

This capability provider implements the "wasmcloud:httpclient" capability
contract using the rust 'reqwest' library

It is multi-threaded and can handle concurrent requests from multiple actors.

Build with 'make'. Test with 'make test'.

