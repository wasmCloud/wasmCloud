# S3 Blobstore Capability Provider

This capability provider is an implementation of the `wasmcloud:blobstore` contract. 
It provides a means to access buckets and files on AWS S3.

## Configuration

- For simple uses, no configuration file is needed, and the standard AWS environment variables should be set:
  - `AWS_ACCESS_KEY` (required)
  - `AWS_SECRET_ACCESS_KEY` (required)
  - `AWS_SESSION_TOKEN` (optional)
  - `AWS_REGION` (optional)

   
- If you intend to use assumed role authentication, the user or role for the above credentials should have an IAM role that is allowed to AssumeRole
  - `AWS_ASSUME_ROLE_ARN` - (required, if using STS AssumedRole Authentication) the role to assume, of the form  "arn:aws:iam::123456789012:role/example". This is the role that should have allowed policies for S3
  - `AWS_ASSUME_ROLE_REGION` - (optional) the region that will be used for the assumed role (for using S3). Note that `AWS_REGION` is the region used for contacting STS
  - `AWS_ASSUME_ROLE_SESSION` - (optional) the session name for the assumed role. Default value is blobstore_s3_provider
  - `AWS_ASSUME_ROLE_EXTERNAL_ID` - (optional) the external id to be associated with the role. This can be used if your auth policy requires a value for externalId

- It's possible to assume a role using a web identity token, using
  - `AWS_WEB_IDENTITY_TOKEN_FILE`, and `AWS_ROLE_ARN` for the service account role.
    See [service-account-role](https://docs.aws.amazon.com/eks/latest/userguide/specify-service-account-role.html) for more information

- Alternately, you may configure settings with a json-format configuration, rather than environment variables, and use a base64-encoded json structure that is loaded with a Link Definition. If you wish to define a json data structure, it must use the schema of `StorageConfig` in src/config.rs.
If you wish to use different S3 provider credentials with different actors, you _must_ use the base64 and json-formatted `StorageConfig` to save in the Link Definition.
 
## Known issues

- getContainerInfo does not return container creation date (it's not available in head_bucket request)
- multipart upload (file size > 995KB) is not implemented.

## Not tested

- multipart download is not tested. (All other blobstore operations have at least one unit test case)
  - no effort has been put into adjusting message/chunk sizes for transfer to the actor. It is possible that the message sizes used are either too small, resulting in needless nats traffic, or too large, resulting in longer network latencies.
- AssumeRole is not tested
  - Automatic Retry on expired session token is not tested
- Although it has not been tested, this provider may be able to work with S3-compatible services such as Minio or Yandex, although only S3 support is tested and officially supported.

## Wish list

- support ifModifiedSince
