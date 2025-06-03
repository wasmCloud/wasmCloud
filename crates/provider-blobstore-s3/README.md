# Blobstore-S3 Capability Provider

This capability provider is an implementation of the `wasmcloud:blobstore` contract.
It provides a means to access buckets and files on AWS S3, and supports simultaneous S3 access
from different components configured with different access roles and policies.

## Configuration

### Via Link Definition (config-json, `config_b64`)

The primary means of configuring Blobstore-S3 to work on a per-link basis is with a Base64 encoded JSON value that is set as link configuration.

This option, like the 'env' file, allows for settings to be specific to a link, however it is not as secure, because of the additional processing required to generate the encoded structure and pass it into either `wash`, or via a wadm application manifest.

> ![NOTE]
> The field names in the JSON structure that is encoded are defined by `StorageConfig` in src/config.rs, they are different from the environment variable names in other sections.

**Base64 encoded JSON settings take precedence over environment variables and 'env' file values.**

For example if we wanted to use the following S3 credentials:

```json
{
  "access_key_id": "XXX",
  "secret_access_key": "YYY",
  "bucket_region":"us-west-1",
}
```

> ![NOTE]
> `bucket_region` is optional -- by default buckets are created in `us-east-1`, but if you specify `bucket_region`, buckets can be created in other regions.

<details>
<summary>See all expected fields of the base64 JSON link configuration payload</summary>

```rust
pub struct StorageConfig {
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>, // AWS only
    pub region: Option<String>,
    pub max_attempts: Option<u32>,
    pub sts_config: Option<StsAssumeRoleConfig>, // AWS only
    pub endpoint: Option<String>,
    pub aliases: HashMap<String, String>,
    pub bucket_region: Option<String>,
}
```

</details>

First we need to convert the above JSON to Base64 -- you can do that with a command line tool like `base64`:

```console
export ENCODED_CONFIG=$(echo '{"access_key_id":....}' | base64);
```

> [!WARN]
> Base64 encoding is *not* encryption. Do not check base64 encoded values into source control.

Then we can save a named configuration with `wash config put`:

```console
wash config put default-s3 config_b64=$ENCODED_CONFIG
```

### Via environment variables/filesystem (AWS only)

> ![WARN]
> Process environment variables apply to all linked components, unless they are overridden by an 'env' file for that link.
>
> Environment variables are also specific to the AWS SDK, so please ensure to use 'AWS_*' ENV variables.
>
> To avoid either of these limitations, use link-definition supplied config, which should work with any S3 compatible object storage.

The standard variables are used for connecting to AWS services:

- `AWS_ACCESS_KEY_ID` (required)
- `AWS_SECRET_ACCESS_KEY` (required)
- `AWS_SESSION_TOKEN` (optional)
- `AWS_REGION` (optional)
- `AWS_ENDPOINT` (optional, static endpoint to override for resolving s3. For local testing purposes only, should not be used in production)

If the credentials are not found in the environment, the following locations are searched:
- `~/.aws/config`, `~/.aws/credentials` (see [Configuration and credential file settings](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-files.html))
- from file named by the environment variable `AWS_WEB_IDENTITY_TOKEN_FILE`
- ECS (IAM Roles for tasks)

### STS Assumed Role Authentication (AWS only)

> ![WARN]
> Process environment variables apply to all linked components, unless they are overridden by an 'env' file for that link.
>
> To avoid this, use link-definition supplied config

If you intend to use STS Assumed Role authentication, the user or role for the above credentials should have an IAM role that is allowed to AssumeRole.

- `AWS_ROLE_ARN` - (required, if using STS AssumedRole Authentication) the role to assume, of the form  "arn:aws:iam::123456789012:role/example". This is the role that should have allowed policies for S3
- `AWS_ROLE_SESSION_NAME` - (optional) the session name for the assumed role. Default value is blobstore_s3_provider
- `AWS_ROLE_REGION` - (optional) the region that will be used for the assumed role (for using S3). Note that `AWS_REGION` is the region used for contacting STS
- `AWS_ROLE_EXTERNAL_ID` - (optional) the external id to be associated with the role. This can be used if your auth policy requires a value for externalId

### ENV file

Blobstore-s3 capability provider settings can be passed to the provider through an env file, as
described above, or through environment variables in the provider's process. Configuring through environment variables
is useful for testing from the command-line, or when the provider and wasmcloud host are running in a k8s container.

> ![WARN]
> Process environment variables apply to all linked components, unless they are overridden by an 'env' file for that link.
>
> To avoid this, use link-definition supplied config

The blobstore-S3 can maintain simultaneous connections for different components using different access roles and policies,
but only if credentials are specified with link parameters (the 'env' file described above,
or 'config-json', below). Process environment variables are not link-specific and so cannot be used to enforce
different access policies. When Blobstore-S3 is expected to provide services to components with distinct
access roles, environment variables should only be used for non-secret settings such as `AWS_REGION`
that may apply to multiple components.

For any settings defined both in an 'env' file and the environment, the value from the 'env' file takes precedence.

## Aliases

Link definitions can optionally contain bucket name aliases which replace an alias with a different name.
For example, if the link definition contains the setting "alias_backup=backup.20220101", then for any api
where the component saves an object to the bucket "backup", it will actually be stored in the bucket "backup.20220101".
The use case for this is to allow the component to hard-code a small number of symbolic names that can be remapped
by an administrator when linking the component to this provider. If an alias is defined, it is in effect for all api methods.
Any use of a bucket name not in the alias map is passed on without change. As a convention, it is recommended
to use the prefix "alias_" for bucket names within component code, to clarify to readers that use of an alias is intended;
however, the prefix is not required.


## Known issues

- getContainerInfo does not return container creation date (it's not available in head_bucket request)
- multipart upload (file size > 995KB) is not implemented.

## Not tested

- AssumeRole is not tested
  - Automatic Retry on expired session token is not tested
- "S3-compatible" services such as Minio or Yandex. There are no plans by the developer to support "S3-compatible" services other than AWS.

## Wish list

- support ifModifiedSince


## Running the Tests

To run `cargo test` successfully, this provider requires either:
1. A local docker setup, so that [testcontainers](https://github.com/testcontainers/testcontainers-rs) can be used to run a [localstack](https://github.com/localstack/localstack) container for S3.
2. AWS configuration (see [Configuration](#Configuration) above)

Then set your environment variables and run the test
```shell
export AWS_REGION=us-east-1
export AWS_ACCESS_KEY_ID=YOUR_AWS_ACCESS_KEY_ID
export AWS_SECRET_ACCESS_KEY=YOUR_AWS_SECRET_ACCESS_KEY
export AWS_ENDPOINT=AWS_ENDPOINT_URL
cargo test
```

Please note that if `AWS_ENDPOINT` environment variable is not set, a [localstack](https://github.com/localstack/localstack) testcontainer will be used instead.