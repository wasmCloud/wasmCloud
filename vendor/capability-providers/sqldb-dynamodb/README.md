# sqldb-dynamodb capability provider

This capability provider allows wasmCloud components to use AWS DynamoDB
through PartiQL, and implements the "wasmcloud:sqldb" capability contract.

Build with 'make'. Test with 'make test'.

### Capabilities:
- execute Parti statements (select, insert, update, etc.)
- query and execute only differ in that query will return the result

## Configuration

- The standard variables are used for connecting to AWS services:
  - `AWS_ACCESS_KEY_ID` (required)
  - `AWS_SECRET_ACCESS_KEY` (required)
  - `AWS_SESSION_TOKEN` (optional)
  - `AWS_REGION` (optional)
  - `AWS_ENDPOINT` (optional, static endpoint to override for resolving DynamoDB AWS Endpoint. For local testing purposes only, should not be used in production)

- If the credentials are not found in the environment, the following locations are searched:
  - `~/.aws/config`, `~/.aws/credentials` (see [Configuration and credential file settings](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-files.html))
  - from file named by the environment variable `AWS_WEB_IDENTITY_TOKEN_FILE`
  - ECS (IAM Roles for tasks)

- If you intend to use STS Assumed Role authentication, the user or role for the above credentials should have an IAM role that is allowed to AssumeRole
  - `AWS_ROLE_ARN` - (required, if using STS AssumedRole Authentication) the role to assume, of the form  "arn:aws:iam::123456789012:role/example". This is the role that should have allowed policies for S3
  - `AWS_ROLE_SESSION_NAME` - (optional) the session name for the assumed role. Default value is sqldb_dynamodb_provider
  - `AWS_ROLE_REGION` - (optional) the region that will be used for the assumed role (for using DynamoDB). Note that `AWS_REGION` is the region used for contacting STS
  - `AWS_ROLE_EXTERNAL_ID` - (optional) the external id to be associated with the role. This can be used if your auth policy requires a value for externalId


### with 'env' file (link definition)

When linking the sqldb-DynamoDB capability provider to a component, you can use the link parameter `env`
to specify the name of the file containing configuration settings.
The value of the `env` parameter should be an absolute path to a text file on disk.

The file should be ascii or UTF-8, and contain one line per variable, with optional comments. The syntax is defined as follows:
```
# Comments are ignored
VAR_NAME = "value"  # sets a string value. spaces around the equals ('=') are optional.
VAR_NAME = value    # quotes around values are optional. This line has the same effect as the previous line.
VAR_NAME="value"    # so does this
```

If a file is used to define settings, and any environment variables are defined for the provider process
_and_ defined in the 'env' file, values from the file take precedence.

### with environment variables

SqlDb-DynamoDB capability provider settings can be passed to the provider through an env file, as
described above, or through environment variables in the provider's process. Configuring through environment variables
is useful for testing from the command-line, or when the provider and wasmcloud host are running in a k8s container.
Note that process environment variables apply to all linked components, unless they are overridden by an 'env' file for that link.

The sqldb-DynamoDB can maintain simultaneous connections for different components using different access roles and policies,
but only if credentials are specified with component link parameters (the 'env' file described above,
or 'config-json', below). Process environment variables are not link-specific and so cannot be used to enforce
different access policies. When Blobstore-S3 is expected to provide services to components with distinct
access roles, environment variables should only be used for non-secret settings such as `AWS_REGION`
that may apply to multiple components.

For any settings defined both in an 'env' file and the environment, the value from the 'env' file takes precedence.

### with config-json (link definition)

A third means of setting Blobstore-S3 configuration is with a json file, base-64 encoded,
and passed as the link value `config_b64`. This option, like the 'env' file, allows for settings
to be specific to a component-link, however it is not as secure, because of the additional processing
required to generate the encoded structure and pass it into either `wash` or the web dashboard.
Note that the field names in the json structure, defined by `StorageConfig` in src/config.rs,
are different from the environment variable names.

Json settings take precedence over environment variables and 'env' file values.
