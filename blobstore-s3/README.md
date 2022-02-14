# S3 Blobstore Capability Provider
This capability provider is an implementation of the `wasmcloud:blobstore` contract. 
It provides a means to access buckets and files on AWS S3.

## Status

- All blobstore interface functions are implemented except for multipart upload. Hence, the maximum size of a file (object) that can be uploaded is slightly under 1MB with the default nats configuration.

- Unit tests are incomplete