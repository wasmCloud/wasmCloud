# S3 Blobstore Capability Provider

This capability provider is an implementation of the `wasmcloud:blobstore` contract. 
It provides a means to access buckets and files on AWS S3.

## Known issues

- getContainerInfo not implemented
- multipart upload (file size > 995KB) is not implemented.

## Not tested

- multipart download is not tested. (All other blobstore operations have at least one unit test case)
- AssumeRole is not tested
  - Automatic Retry on expired session token is not tested

## Wish list

- add contentType, contentEncoding to ObjectMetadata
- support ifModifiedSince