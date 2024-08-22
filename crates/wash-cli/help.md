# Command-Line Help for `wash`

This document contains the help content for the `wash` command-line program.

**Command Overview:**

* [`wash`↴](#wash)
* [`wash app`↴](#wash-app)
* [`wash app list`↴](#wash-app-list)
* [`wash app get`↴](#wash-app-get)
* [`wash app status`↴](#wash-app-status)
* [`wash app history`↴](#wash-app-history)
* [`wash app delete`↴](#wash-app-delete)
* [`wash app put`↴](#wash-app-put)
* [`wash app deploy`↴](#wash-app-deploy)
* [`wash app undeploy`↴](#wash-app-undeploy)
* [`wash app validate`↴](#wash-app-validate)
* [`wash build`↴](#wash-build)
* [`wash call`↴](#wash-call)
* [`wash capture`↴](#wash-capture)
* [`wash capture replay`↴](#wash-capture-replay)
* [`wash completions`↴](#wash-completions)
* [`wash completions zsh`↴](#wash-completions-zsh)
* [`wash completions bash`↴](#wash-completions-bash)
* [`wash completions fish`↴](#wash-completions-fish)
* [`wash completions power-shell`↴](#wash-completions-power-shell)
* [`wash claims`↴](#wash-claims)
* [`wash claims inspect`↴](#wash-claims-inspect)
* [`wash claims sign`↴](#wash-claims-sign)
* [`wash claims token`↴](#wash-claims-token)
* [`wash claims token component`↴](#wash-claims-token-component)
* [`wash claims token operator`↴](#wash-claims-token-operator)
* [`wash claims token account`↴](#wash-claims-token-account)
* [`wash claims token provider`↴](#wash-claims-token-provider)
* [`wash config`↴](#wash-config)
* [`wash config put`↴](#wash-config-put)
* [`wash config get`↴](#wash-config-get)
* [`wash config del`↴](#wash-config-del)
* [`wash ctx`↴](#wash-ctx)
* [`wash ctx list`↴](#wash-ctx-list)
* [`wash ctx del`↴](#wash-ctx-del)
* [`wash ctx new`↴](#wash-ctx-new)
* [`wash ctx default`↴](#wash-ctx-default)
* [`wash ctx edit`↴](#wash-ctx-edit)
* [`wash dev`↴](#wash-dev)
* [`wash down`↴](#wash-down)
* [`wash drain`↴](#wash-drain)
* [`wash drain all`↴](#wash-drain-all)
* [`wash drain oci`↴](#wash-drain-oci)
* [`wash drain lib`↴](#wash-drain-lib)
* [`wash drain downloads`↴](#wash-drain-downloads)
* [`wash get`↴](#wash-get)
* [`wash get links`↴](#wash-get-links)
* [`wash get claims`↴](#wash-get-claims)
* [`wash get hosts`↴](#wash-get-hosts)
* [`wash get inventory`↴](#wash-get-inventory)
* [`wash inspect`↴](#wash-inspect)
* [`wash keys`↴](#wash-keys)
* [`wash keys gen`↴](#wash-keys-gen)
* [`wash keys get`↴](#wash-keys-get)
* [`wash keys list`↴](#wash-keys-list)
* [`wash link`↴](#wash-link)
* [`wash link query`↴](#wash-link-query)
* [`wash link put`↴](#wash-link-put)
* [`wash link del`↴](#wash-link-del)
* [`wash new`↴](#wash-new)
* [`wash new component`↴](#wash-new-component)
* [`wash new provider`↴](#wash-new-provider)
* [`wash par`↴](#wash-par)
* [`wash par create`↴](#wash-par-create)
* [`wash par inspect`↴](#wash-par-inspect)
* [`wash par insert`↴](#wash-par-insert)
* [`wash plugin`↴](#wash-plugin)
* [`wash plugin install`↴](#wash-plugin-install)
* [`wash plugin uninstall`↴](#wash-plugin-uninstall)
* [`wash plugin list`↴](#wash-plugin-list)
* [`wash push`↴](#wash-push)
* [`wash pull`↴](#wash-pull)
* [`wash secrets`↴](#wash-secrets)
* [`wash secrets put`↴](#wash-secrets-put)
* [`wash secrets get`↴](#wash-secrets-get)
* [`wash secrets del`↴](#wash-secrets-del)
* [`wash spy`↴](#wash-spy)
* [`wash scale`↴](#wash-scale)
* [`wash scale component`↴](#wash-scale-component)
* [`wash start`↴](#wash-start)
* [`wash start component`↴](#wash-start-component)
* [`wash start provider`↴](#wash-start-provider)
* [`wash stop`↴](#wash-stop)
* [`wash stop component`↴](#wash-stop-component)
* [`wash stop provider`↴](#wash-stop-provider)
* [`wash stop host`↴](#wash-stop-host)
* [`wash label`↴](#wash-label)
* [`wash update`↴](#wash-update)
* [`wash update component`↴](#wash-update-component)
* [`wash up`↴](#wash-up)
* [`wash ui`↴](#wash-ui)

## `wash`

**Usage:** `wash [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `app` — Manage declarative applications and deployments (wadm)
* `build` — Build (and sign) a wasmCloud component or capability provider
* `call` — Invoke a simple function on a component running in a wasmCloud host
* `capture` — Capture and debug cluster invocations and state
* `completions` — Generate shell completions
* `claims` — Generate and manage JWTs for wasmCloud components and capability providers
* `config` — Create configuration for components, capability providers and links
* `ctx` — Manage wasmCloud host configuration contexts
* `dev` — Start a developer loop to hot-reload a local wasmCloud component
* `down` — Tear down a wasmCloud environment launched with wash up
* `drain` — Manage contents of local wasmCloud caches
* `get` — Get information about different running wasmCloud resources
* `inspect` — Inspect a capability provider or Wasm component for signing information and interfaces
* `keys` — Utilities for generating and managing signing keys
* `link` — Link one component to another on a set of interfaces
* `new` — Create a new project from a template
* `par` — Create, inspect, and modify capability provider archive files
* `plugin` — Manage wash plugins
* `push` — Push an artifact to an OCI compliant registry
* `pull` — Pull an artifact from an OCI compliant registry
* `secrets` — Manage secret references
* `spy` — Spy on all invocations a component sends and receives
* `scale` — Scale a component running in a host to a certain level of concurrency
* `start` — Start a component or capability provider
* `stop` — Stop a component, capability provider, or host
* `label` — Label (or un-label) a host with a key=value label pair
* `update` — Update a component running in a host to newer image reference
* `up` — Bootstrap a wasmCloud environment
* `ui` — Serve a web UI for wasmCloud

###### **Options:**

* `-o`, `--output <OUTPUT>` — Specify output format (text or json)

  Default value: `text`
* `--experimental` — Whether or not to enable experimental features

  Default value: `false`



## `wash app`

Manage declarative applications and deployments (wadm)

**Usage:** `wash app <COMMAND>`

###### **Subcommands:**

* `list` — List all applications available within the lattice
* `get` — Get the application manifest for a specific version of an application
* `status` — Get the current status of a given application
* `history` — Get the version history of a given application
* `delete` — Delete an application version
* `put` — Create an application version by putting the manifest into the wadm store
* `deploy` — Deploy an application to the lattice
* `undeploy` — Undeploy an application, removing it from the lattice
* `validate` — Validate an application manifest



## `wash app list`

List all applications available within the lattice

**Usage:** `wash app list [OPTIONS]`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app get`

Get the application manifest for a specific version of an application

**Usage:** `wash app get [OPTIONS] <name> [version]`

###### **Arguments:**

* `<name>` — The name of the application to retrieve
* `<version>` — The version of the application to retrieve. If left empty, retrieves the latest version

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app status`

Get the current status of a given application

**Usage:** `wash app status [OPTIONS] <name>`

###### **Arguments:**

* `<name>` — The name of the application

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app history`

Get the version history of a given application

**Usage:** `wash app history [OPTIONS] <name>`

###### **Arguments:**

* `<name>` — The name of the application

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app delete`

Delete an application version

**Usage:** `wash app delete [OPTIONS] <name> [version]`

###### **Arguments:**

* `<name>` — Name of the application to delete, or a path to a Wadm Application Manifest
* `<version>` — Version of the application to delete. If not supplied, all versions are deleted

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app put`

Create an application version by putting the manifest into the wadm store

**Usage:** `wash app put [OPTIONS] [SOURCE]`

###### **Arguments:**

* `<SOURCE>` — The source of the application manifest, either a file path, remote file http url, or stdin. If no source is provided (or arg marches '-'), stdin is used

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app deploy`

Deploy an application to the lattice

**Usage:** `wash app deploy [OPTIONS] [application] [version]`

###### **Arguments:**

* `<application>` — Name of the application to deploy, if it was already `put`, or a path to a file containing the application manifest
* `<version>` — Version of the application to deploy, defaults to the latest created version

###### **Options:**

* `--replace` — Whether or not wash should attempt to replace the resources by performing an optimistic delete shortly before applying resources
* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app undeploy`

Undeploy an application, removing it from the lattice

**Usage:** `wash app undeploy [OPTIONS] <name>`

###### **Arguments:**

* `<name>` — Name of the application to undeploy

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash app validate`

Validate an application manifest

**Usage:** `wash app validate <application>`

###### **Arguments:**

* `<application>` — Path to the application manifest to validate



## `wash build`

Build (and sign) a wasmCloud component or capability provider

**Usage:** `wash build [OPTIONS]`

###### **Options:**

* `-p`, `--config-path <CONFIG_PATH>` — Path to the wasmcloud.toml file or parent folder to use for building
* `--keys-directory <KEYS_DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (account). If this flag is not provided, the seed will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (module or service). If this flag is not provided, the seed will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `--disable-keygen` — Disables autogeneration of keys if seed(s) are not provided
* `--build-only` — Skip signing the artifact and only use the native toolchain to build
* `--sign-only` — Skip building the artifact and only use configuration to sign



## `wash call`

Invoke a simple function on a component running in a wasmCloud host

**Usage:** `wash call [OPTIONS] <component-id> <function>`

###### **Arguments:**

* `<component-id>` — The unique component identifier of the component to invoke
* `<function>` — Fully qualified WIT export to invoke on the component, e.g. `wasi:cli/run.run`

###### **Options:**

* `-r`, `--rpc-host <RPC_HOST>` — RPC Host for connection, defaults to 127.0.0.1 for local nats

  Default value: `127.0.0.1`
* `-p`, `--rpc-port <RPC_PORT>` — RPC Port for connections, defaults to 4222 for local nats

  Default value: `4222`
* `--rpc-jwt <RPC_JWT>` — JWT file for RPC authentication. Must be supplied with rpc_seed
* `--rpc-seed <RPC_SEED>` — Seed file or literal for RPC authentication. Must be supplied with rpc_jwt
* `--rpc-credsfile <RPC_CREDSFILE>` — Credsfile for RPC authentication. Combines rpc_seed and rpc_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--rpc-ca-file <RPC_CA_FILE>` — CA file for RPC authentication. See https://docs.nats.io/using-nats/developer/security/securing_nats for details
* `-x`, `--lattice <LATTICE>` — Lattice for wasmcloud command interface, defaults to "default"
* `-t`, `--rpc-timeout-ms <TIMEOUT_MS>` — Timeout length for RPC, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of the context to use for RPC connection, authentication, and cluster seed invocation signing
* `--http-response-extract-json` — Whether the content of the HTTP response body should be parsed as JSON and returned directly

  Default value: `false`
* `--http-scheme <HTTP_SCHEME>` — Scheme to use when making the HTTP request
* `--http-host <HTTP_HOST>` — Host to use when making the HTTP request
* `--http-port <HTTP_PORT>` — Port on which to make the HTTP request
* `--http-method <HTTP_METHOD>` — Method to use when making the HTTP request
* `--http-body <HTTP_BODY>` — Stringified body contents to use when making the HTTP request
* `--http-body-path <HTTP_BODY_PATH>` — Path to a file to use as the body when making a HTTP request
* `--http-content-type <HTTP_CONTENT_TYPE>` — Content type header to pass with the request



## `wash capture`

Capture and debug cluster invocations and state

**Usage:** `wash capture [OPTIONS] [COMMAND]`

###### **Subcommands:**

* `replay` — 

###### **Options:**

* `--enable` — Enable wash capture. This will setup a NATS JetStream stream to capture all invocations
* `--disable` — Disable wash capture. This will removed the NATS JetStream stream that was setup to capture all invocations
* `--window-size <window_size>` — The length of time in minutes to keep messages in the stream

  Default value: `60`
* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash capture replay`

**Usage:** `wash capture replay [OPTIONS] <capturefile>`

###### **Arguments:**

* `<capturefile>` — The file path to the capture file to read from

###### **Options:**

* `--source-id <source_id>` — A component ID to filter captured invocations by. This will filter anywhere the component is the source of the invocation
* `--target-id <target_id>` — A component ID to filter captured invocations by. This will filter anywhere the component is the target of the invocation
* `--interactive` — Whether or not to step through the replay one message at a time



## `wash completions`

Generate shell completions

**Usage:** `wash completions [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `zsh` — generate completions for Zsh
* `bash` — generate completions for Bash
* `fish` — generate completions for Fish
* `power-shell` — generate completions for PowerShell

###### **Options:**

* `-d`, `--dir <DIR>` — Output directory (default '.')



## `wash completions zsh`

generate completions for Zsh

**Usage:** `wash completions zsh`



## `wash completions bash`

generate completions for Bash

**Usage:** `wash completions bash`



## `wash completions fish`

generate completions for Fish

**Usage:** `wash completions fish`



## `wash completions power-shell`

generate completions for PowerShell

**Usage:** `wash completions power-shell`



## `wash claims`

Generate and manage JWTs for wasmCloud components and capability providers

**Usage:** `wash claims <COMMAND>`

###### **Subcommands:**

* `inspect` — Examine the signing claims information or WIT world from a signed component component
* `sign` — Sign a WebAssembly component, specifying capabilities and other claims including expiration, tags, and additional metadata
* `token` — Generate a signed JWT by supplying basic token information, a signing seed key, and metadata



## `wash claims inspect`

Examine the signing claims information or WIT world from a signed component component

**Usage:** `wash claims inspect [OPTIONS] <COMPONENT>`

###### **Arguments:**

* `<COMPONENT>` — Path to signed component or OCI URL of signed component

###### **Options:**

* `--jwt-only` — Extract the raw JWT from the file and print to stdout
* `--wit` — Extract the WIT world from a component and print to stdout instead of the claims
* `-d`, `--digest <DIGEST>` — Digest to verify artifact against (if OCI URL is provided for <component>)
* `--allow-latest` — Allow latest artifact tags (if OCI URL is provided for <component>)
* `-u`, `--user <USER>` — OCI username, if omitted anonymous authentication will be used
* `-p`, `--password <PASSWORD>` — OCI password, if omitted anonymous authentication will be used
* `--insecure` — Allow insecure (HTTP) registry connections
* `--insecure-skip-tls-verify` — Skip checking OCI registry's certificate for validity
* `--no-cache` — skip the local OCI cache



## `wash claims sign`

Sign a WebAssembly component, specifying capabilities and other claims including expiration, tags, and additional metadata

**Usage:** `wash claims sign [OPTIONS] <SOURCE>`

###### **Arguments:**

* `<SOURCE>` — File to read

###### **Options:**

* `-d`, `--destination <DESTINATION>` — Destination for signed module. If this flag is not provided, the signed module will be placed in the same directory as the source with a "_s" suffix
* `-n`, `--name <NAME>` — A human-readable, descriptive name for the token
* `-t`, `--tag <TAGS>` — A list of arbitrary tags to be embedded in the token
* `-r`, `--rev <REV>` — Revision number
* `-v`, `--ver <VER>` — Human-readable version string
* `-a`, `--call-alias <CALL_ALIAS>` — Developer or human friendly unique alias used for invoking an component, consisting of lowercase alphanumeric characters, underscores '_' and slashes '/'
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (module). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-x`, `--expires <EXPIRES_IN_DAYS>` — Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
* `-b`, `--nbf <NOT_BEFORE_DAYS>` — Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
* `--disable-keygen` — Disables autogeneration of keys if seed(s) are not provided



## `wash claims token`

Generate a signed JWT by supplying basic token information, a signing seed key, and metadata

**Usage:** `wash claims token <COMMAND>`

###### **Subcommands:**

* `component` — Generate a signed JWT for an component module
* `operator` — Generate a signed JWT for an operator
* `account` — Generate a signed JWT for an account
* `provider` — Generate a signed JWT for a service (capability provider)



## `wash claims token component`

Generate a signed JWT for an component module

**Usage:** `wash claims token component [OPTIONS]`

###### **Options:**

* `-n`, `--name <NAME>` — A human-readable, descriptive name for the token
* `-t`, `--tag <TAGS>` — A list of arbitrary tags to be embedded in the token
* `-r`, `--rev <REV>` — Revision number
* `-v`, `--ver <VER>` — Human-readable version string
* `-a`, `--call-alias <CALL_ALIAS>` — Developer or human friendly unique alias used for invoking an component, consisting of lowercase alphanumeric characters, underscores '_' and slashes '/'
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (module). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-x`, `--expires <EXPIRES_IN_DAYS>` — Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
* `-b`, `--nbf <NOT_BEFORE_DAYS>` — Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
* `--disable-keygen` — Disables autogeneration of keys if seed(s) are not provided



## `wash claims token operator`

Generate a signed JWT for an operator

**Usage:** `wash claims token operator [OPTIONS] --name <NAME>`

###### **Options:**

* `-n`, `--name <NAME>` — A descriptive name for the operator
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (self signing operator). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-a`, `--additional-key <additional-keys>` — Additional keys to add to valid signers list Can either be seed value or path to seed file
* `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-x`, `--expires <EXPIRES_IN_DAYS>` — Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
* `-b`, `--nbf <NOT_BEFORE_DAYS>` — Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
* `--disable-keygen` — Disables autogeneration of keys if seed(s) are not provided



## `wash claims token account`

Generate a signed JWT for an account

**Usage:** `wash claims token account [OPTIONS] --name <NAME>`

###### **Options:**

* `-n`, `--name <NAME>` — A descriptive name for the account
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (operator). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-a`, `--additional-key <additional-keys>` — Additional keys to add to valid signers list. Can either be seed value or path to seed file
* `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-x`, `--expires <EXPIRES_IN_DAYS>` — Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
* `-b`, `--nbf <NOT_BEFORE_DAYS>` — Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
* `--disable-keygen` — Disables autogeneration of keys if seed(s) are not provided



## `wash claims token provider`

Generate a signed JWT for a service (capability provider)

**Usage:** `wash claims token provider [OPTIONS]`

###### **Options:**

* `-n`, `--name <NAME>` — A descriptive name for the provider
* `-v`, `--vendor <VENDOR>` — A human-readable string identifying the vendor of this provider (e.g. Redis or Cassandra or NATS etc)
* `-r`, `--revision <REVISION>` — Monotonically increasing revision number
* `-e`, `--version <VERSION>` — Human-friendly version string
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (service). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-x`, `--expires <EXPIRES_IN_DAYS>` — Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
* `-b`, `--nbf <NOT_BEFORE_DAYS>` — Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
* `--disable-keygen` — Disables autogeneration of keys if seed(s) are not provided



## `wash config`

Create configuration for components, capability providers and links

**Usage:** `wash config <COMMAND>`

###### **Subcommands:**

* `put` — Put named configuration
* `get` — Get a named configuration
* `del` — Delete a named configuration



## `wash config put`

Put named configuration

**Usage:** `wash config put [OPTIONS] <name> <config_value>...`

###### **Arguments:**

* `<name>` — The name of the configuration to put
* `<config_value>` — The configuration values to put, in the form of `key=value`. Can be specified multiple times, but must be specified at least once

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash config get`

Get a named configuration

**Usage:** `wash config get [OPTIONS] <name>`

###### **Arguments:**

* `<name>` — The name of the configuration to get

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash config del`

Delete a named configuration

**Usage:** `wash config del [OPTIONS] <name>`

###### **Arguments:**

* `<name>` — The name of the configuration to delete

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash ctx`

Manage wasmCloud host configuration contexts

**Usage:** `wash ctx <COMMAND>`

###### **Subcommands:**

* `list` — Lists all stored contexts (JSON files) found in the context directory, with the exception of index.json
* `del` — Delete a stored context
* `new` — Create a new context
* `default` — Set the default context
* `edit` — Edit a context directly using a text editor



## `wash ctx list`

Lists all stored contexts (JSON files) found in the context directory, with the exception of index.json

**Usage:** `wash ctx list [OPTIONS]`

###### **Options:**

* `--directory <DIRECTORY>` — Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)



## `wash ctx del`

Delete a stored context

**Usage:** `wash ctx del [OPTIONS] [name]`

###### **Arguments:**

* `<name>` — Name of the context to delete. If not supplied, the user will be prompted to select an existing context

###### **Options:**

* `--directory <DIRECTORY>` — Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)



## `wash ctx new`

Create a new context

**Usage:** `wash ctx new [OPTIONS] [name]`

###### **Arguments:**

* `<name>` — Name of the context, will be sanitized to ensure it's a valid filename

###### **Options:**

* `--directory <DIRECTORY>` — Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
* `-i`, `--interactive` — Create the context in an interactive terminal prompt, instead of an autogenerated default context



## `wash ctx default`

Set the default context

**Usage:** `wash ctx default [OPTIONS] [name]`

###### **Arguments:**

* `<name>` — Name of the context to use for default. If not supplied, the user will be prompted to select a default

###### **Options:**

* `--directory <DIRECTORY>` — Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)



## `wash ctx edit`

Edit a context directly using a text editor

**Usage:** `wash ctx edit [OPTIONS] --editor <EDITOR> [name]`

###### **Arguments:**

* `<name>` — Name of the context to edit, if not supplied the user will be prompted to select a context

###### **Options:**

* `--directory <DIRECTORY>` — Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
* `-e`, `--editor <EDITOR>` — Your terminal text editor of choice. This editor must be present in your $PATH, or an absolute filepath



## `wash dev`

Start a developer loop to hot-reload a local wasmCloud component

**Usage:** `wash dev [OPTIONS]`

###### **Options:**

* `--nats-credsfile <NATS_CREDSFILE>` — Optional path to a NATS credentials file to authenticate and extend existing NATS infrastructure
* `--nats-config-file <NATS_CONFIGFILE>` — Optional path to a NATS config file NOTE: If your configuration changes the address or port to listen on from 0.0.0.0:4222, ensure you set --nats-host and --nats-port
* `--nats-remote-url <NATS_REMOTE_URL>` — Optional remote URL of existing NATS infrastructure to extend
* `--nats-connect-only` — If a connection can't be established, exit and don't start a NATS server. Will be ignored if a remote_url and credsfile are specified
* `--nats-version <NATS_VERSION>` — NATS server version to download, e.g. `v2.10.7`. See https://github.com/nats-io/nats-server/releases/ for releases

  Default value: `v2.10.7`
* `--nats-host <NATS_HOST>` — NATS server host to connect to
* `--nats-port <NATS_PORT>` — NATS server port to connect to. This will be used as the NATS listen port if `--nats-connect-only` isn't set
* `--nats-websocket-port <NATS_WEBSOCKET_PORT>` — NATS websocket port to use. TLS is not supported. This is required for the wash ui to connect from localhost

  Default value: `4223`
* `--nats-js-domain <NATS_JS_DOMAIN>` — NATS Server Jetstream domain for extending superclusters
* `--wasmcloud-version <WASMCLOUD_VERSION>` — wasmCloud host version to download, e.g. `v0.55.0`. See https://github.com/wasmCloud/wasmcloud/releases for releases

  Default value: `v1.1.0`
* `-x`, `--lattice <LATTICE>` — A unique identifier for a lattice, frequently used within NATS topics to isolate messages among different lattices
* `--host-seed <HOST_SEED>` — The seed key (a printable 256-bit Ed25519 private key) used by this host to generate it's public key
* `--rpc-host <RPC_HOST>` — An IP address or DNS name to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-host if not supplied
* `--rpc-port <RPC_PORT>` — A port to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-port if not supplied
* `--rpc-seed <RPC_SEED>` — A seed nkey to use to authenticate to NATS for RPC messages
* `--rpc-timeout-ms <RPC_TIMEOUT_MS>` — Timeout in milliseconds for all RPC calls

  Default value: `2000`
* `--rpc-jwt <RPC_JWT>` — A user JWT to use to authenticate to NATS for RPC messages
* `--rpc-tls` — Optional flag to enable host communication with a NATS server over TLS for RPC messages
* `--rpc-tls-ca-file <RPC_TLS_CA_FILE>` — A TLS CA file to use to authenticate to NATS for RPC messages
* `--rpc-credsfile <RPC_CREDSFILE>` — Convenience flag for RPC authentication, internally this parses the JWT and seed from the credsfile
* `--ctl-host <CTL_HOST>` — An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
* `--ctl-port <CTL_PORT>` — A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
* `--ctl-seed <CTL_SEED>` — A seed nkey to use to authenticate to NATS for CTL messages
* `--ctl-jwt <CTL_JWT>` — A user JWT to use to authenticate to NATS for CTL messages
* `--ctl-credsfile <CTL_CREDSFILE>` — Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
* `--ctl-tls` — Optional flag to enable host communication with a NATS server over TLS for CTL messages
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — A TLS CA file to use to authenticate to NATS for CTL messages
* `--cluster-seed <CLUSTER_SEED>` — The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
* `--cluster-issuers <CLUSTER_ISSUERS>` — A comma-delimited list of public keys that can be used as issuers on signed invocations
* `--provider-delay <PROVIDER_DELAY>` — Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process

  Default value: `300`
* `--allow-latest` — Determines whether OCI images tagged latest are allowed to be pulled from OCI registries and started
* `--allowed-insecure <ALLOWED_INSECURE>` — A comma-separated list of OCI hosts to which insecure (non-TLS) connections are allowed
* `--wasmcloud-js-domain <WASMCLOUD_JS_DOMAIN>` — Jetstream domain name, configures a host to properly connect to a NATS supercluster
* `--config-service-enabled` — Denotes if a wasmCloud host should issue requests to a config service on startup
* `--allow-file-load <ALLOW_FILE_LOAD>` — Denotes if a wasmCloud host should allow starting components from the file system

  Default value: `true`

  Possible values: `true`, `false`

* `--enable-structured-logging` — Enable JSON structured logging from the wasmCloud host
* `-l`, `--label <LABEL>` — A label to apply to the host, in the form of `key=value`. This flag can be repeated to supply multiple labels
* `--log-level <STRUCTURED_LOG_LEVEL>` — Controls the verbosity of JSON structured logs from the wasmCloud host

  Default value: `info`
* `--enable-ipv6` — Enables IPV6 addressing for wasmCloud hosts
* `--wasmcloud-start-only` — If enabled, wasmCloud will not be downloaded if it's not installed
* `--multi-local` — If enabled, allows starting additional wasmCloud hosts on this machine
* `--max-execution-time-ms <MAX_EXECUTION_TIME>` — Defines the Max Execution time (in ms) that the host runtime will execute for

  Default value: `600000`
* `--wadm-version <WADM_VERSION>` — wadm version to download, e.g. `v0.4.0`. See https://github.com/wasmCloud/wadm/releases for releases

  Default value: `v0.13.0`
* `--disable-wadm` — If enabled, wadm will not be downloaded or run as a part of the up command
* `--wadm-js-domain <WADM_JS_DOMAIN>` — The JetStream domain to use for wadm
* `--wadm-manifest <WADM_MANIFEST>` — The path to a wadm application manifest to run while the host is up
* `--host-id <host-id>` — ID of the host to use for `wash dev` if one is not selected, `wash dev` will attempt to use the single host in the lattice
* `--work-dir <code-dir>` — Path to code directory
* `--leave-host-running` — Leave the wasmCloud host running after stopping the devloop

  Default value: `false`
* `--use-host-subprocess` — Run the wasmCloud host in a subprocess (rather than detached mode)

  Default value: `false`



## `wash down`

Tear down a wasmCloud environment launched with wash up

**Usage:** `wash down [OPTIONS]`

###### **Options:**

* `-x`, `--lattice <LATTICE>` — A lattice prefix is a unique identifier for a lattice, and is frequently used within NATS topics to isolate messages from different lattices

  Default value: `default`
* `--ctl-host <CTL_HOST>` — An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
* `--ctl-port <CTL_PORT>` — A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
* `--ctl-credsfile <CTL_CREDSFILE>` — Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
* `--ctl-seed <CTL_SEED>` — A seed nkey to use to authenticate to NATS for CTL messages
* `--ctl-jwt <CTL_JWT>` — A user JWT to use to authenticate to NATS for CTL messages
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — A TLS CA file to use to authenticate to NATS for CTL messages
* `--host-id <HOST_ID>`
* `--all` — Shutdown all hosts running locally if launched with --multi-local
* `--purge-jetstream <PURGE>` — Purge NATS Jetstream storage and streams that persist when wasmCloud is stopped

  Default value: `none`

  Possible values:
  - `none`:
    Don't purge any Jetstream data, the default
  - `all`:
    Purge all streams and KV buckets for wasmCloud and wadm
  - `wadm`:
    Purge all streams and KV buckets for wadm, removing all application manifests
  - `wasmcloud`:
    Purge all KV buckets for wasmCloud, removing all links and configuration data




## `wash drain`

Manage contents of local wasmCloud caches

**Usage:** `wash drain <COMMAND>`

###### **Subcommands:**

* `all` — Remove all cached files created by wasmcloud
* `oci` — Remove cached files downloaded from OCI registries by wasmCloud
* `lib` — Remove cached binaries extracted from provider archives
* `downloads` — Remove downloaded and generated files from launching wasmCloud hosts



## `wash drain all`

Remove all cached files created by wasmcloud

**Usage:** `wash drain all`



## `wash drain oci`

Remove cached files downloaded from OCI registries by wasmCloud

**Usage:** `wash drain oci`



## `wash drain lib`

Remove cached binaries extracted from provider archives

**Usage:** `wash drain lib`



## `wash drain downloads`

Remove downloaded and generated files from launching wasmCloud hosts

**Usage:** `wash drain downloads`



## `wash get`

Get information about different running wasmCloud resources

**Usage:** `wash get <COMMAND>`

###### **Subcommands:**

* `links` — Retrieve all known links in the lattice
* `claims` — Retrieve all known claims inside the lattice
* `hosts` — Retrieve all responsive hosts in the lattice
* `inventory` — Retrieve inventory a given host on in the lattice



## `wash get links`

Retrieve all known links in the lattice

**Usage:** `wash get links [OPTIONS]`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash get claims`

Retrieve all known claims inside the lattice

**Usage:** `wash get claims [OPTIONS]`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash get hosts`

Retrieve all responsive hosts in the lattice

**Usage:** `wash get hosts [OPTIONS]`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash get inventory`

Retrieve inventory a given host on in the lattice

**Usage:** `wash get inventory [OPTIONS] [host-id]`

###### **Arguments:**

* `<host-id>` — Host ID to retrieve inventory for. If not provided, wash will query the inventories of all running hosts

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash inspect`

Inspect a capability provider or Wasm component for signing information and interfaces

**Usage:** `wash inspect [OPTIONS] <TARGET>`

###### **Arguments:**

* `<TARGET>` — Path or OCI URL to signed component or provider archive

###### **Options:**

* `--jwt-only` — Extract the raw JWT from the file and print to stdout
* `--wit` — Extract the WIT world from a component and print to stdout instead of the claims. When inspecting a provider archive, this flag will be ignored
* `-d`, `--digest <DIGEST>` — Digest to verify artifact against (if OCI URL is provided for <target>)
* `--allow-latest` — Allow latest artifact tags (if OCI URL is provided for <target>)
* `-u`, `--user <USER>` — OCI username, if omitted anonymous authentication will be used
* `-p`, `--password <PASSWORD>` — OCI password, if omitted anonymous authentication will be used
* `--insecure` — Allow insecure (HTTP) registry connections
* `--insecure-skip-tls-verify` — Skip checking OCI registry's certificate for validity
* `--no-cache` — skip the local OCI cache and pull the artifact from the registry to inspect



## `wash keys`

Utilities for generating and managing signing keys

**Usage:** `wash keys <COMMAND>`

###### **Subcommands:**

* `gen` — Generates a keypair
* `get` — Retrieves a keypair and prints the contents
* `list` — Lists all keypairs in a directory



## `wash keys gen`

Generates a keypair

**Usage:** `wash keys gen <KEYTYPE>`

###### **Arguments:**

* `<KEYTYPE>` — The type of keypair to generate. May be Account, User, Module (or Component), Service (or Provider), Server (or Host), Operator, Cluster, Curve (xkey)



## `wash keys get`

Retrieves a keypair and prints the contents

**Usage:** `wash keys get [OPTIONS] <KEYNAME>`

###### **Arguments:**

* `<KEYNAME>` — The name of the key to output

###### **Options:**

* `-d`, `--directory <DIRECTORY>` — Absolute path to where keypairs are stored. Defaults to `$HOME/.wash/keys`



## `wash keys list`

Lists all keypairs in a directory

**Usage:** `wash keys list [OPTIONS]`

###### **Options:**

* `-d`, `--directory <DIRECTORY>` — Absolute path to where keypairs are stored. Defaults to `$HOME/.wash/keys`



## `wash link`

Link one component to another on a set of interfaces

**Usage:** `wash link <COMMAND>`

###### **Subcommands:**

* `query` — Query all links, same as `wash get links`
* `put` — Put a link from a source to a target on a given WIT interface
* `del` — Delete a link



## `wash link query`

Query all links, same as `wash get links`

**Usage:** `wash link query [OPTIONS]`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash link put`

Put a link from a source to a target on a given WIT interface

**Usage:** `wash link put [OPTIONS] --interface <INTERFACES> <source-id> <target> <wit-namespace> <wit-package>`

###### **Arguments:**

* `<source-id>` — The ID of the component to link from
* `<target>` — The ID of the component to link to
* `<wit-namespace>` — The WIT namespace of the link, e.g. "wasi" in "wasi:http/incoming-handler"
* `<wit-package>` — The WIT package of the link, e.g. "http" in "wasi:http/incoming-handler"

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--interface <INTERFACES>` — The interface of the link, e.g. "incoming-handler" in "wasi:http/incoming-handler"
* `--source-config <SOURCE_CONFIG>` — List of named configuration to make available to the source
* `--target-config <TARGET_CONFIG>` — List of named configuration to make available to the target
* `-l`, `--link-name <LINK_NAME>` — Link name, defaults to "default". Used for scenarios where a single source may have multiple links to the same target, or different targets with the same WIT namespace, package, and interface



## `wash link del`

Delete a link

**Usage:** `wash link del [OPTIONS] <source-id> <wit-namespace> <wit-package>`

###### **Arguments:**

* `<source-id>` — Component ID or name of the source of the link
* `<wit-namespace>` — WIT namespace of the link
* `<wit-package>` — WIT package of the link

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `-l`, `--link-name <LINK_NAME>` — Link name, defaults to "default"



## `wash new`

Create a new project from a template

**Usage:** `wash new <COMMAND>`

###### **Subcommands:**

* `component` — Generate a wasmCloud component project
* `provider` — Generate a new capability provider project



## `wash new component`

Generate a wasmCloud component project

**Usage:** `wash new component [OPTIONS] [PROJECT_NAME]`

###### **Arguments:**

* `<PROJECT_NAME>` — Project name

###### **Options:**

* `--git <GIT>` — Github repository url. Requires 'git' to be installed in PATH
* `--subfolder <SUBFOLDER>` — Optional subfolder of the git repository
* `--branch <BRANCH>` — Optional github branch. Defaults to "main"
* `-p`, `--path <PATH>` — Optional path for template project (alternative to --git)
* `-v`, `--values <VALUES>` — Optional path to file containing placeholder values
* `--silent` — Silent - do not prompt user. Placeholder values in the templates will be resolved from a '--values' file and placeholder defaults
* `--favorites <FAVORITES>` — Favorites file - to use for project selection
* `-t`, `--template-name <TEMPLATE_NAME>` — Template name - name of template to use
* `--no-git-init` — Don't run 'git init' on the new folder



## `wash new provider`

Generate a new capability provider project

**Usage:** `wash new provider [OPTIONS] [PROJECT_NAME]`

###### **Arguments:**

* `<PROJECT_NAME>` — Project name

###### **Options:**

* `--git <GIT>` — Github repository url. Requires 'git' to be installed in PATH
* `--subfolder <SUBFOLDER>` — Optional subfolder of the git repository
* `--branch <BRANCH>` — Optional github branch. Defaults to "main"
* `-p`, `--path <PATH>` — Optional path for template project (alternative to --git)
* `-v`, `--values <VALUES>` — Optional path to file containing placeholder values
* `--silent` — Silent - do not prompt user. Placeholder values in the templates will be resolved from a '--values' file and placeholder defaults
* `--favorites <FAVORITES>` — Favorites file - to use for project selection
* `-t`, `--template-name <TEMPLATE_NAME>` — Template name - name of template to use
* `--no-git-init` — Don't run 'git init' on the new folder



## `wash par`

Create, inspect, and modify capability provider archive files

**Usage:** `wash par <COMMAND>`

###### **Subcommands:**

* `create` — Build a provider archive file
* `inspect` — Inspect a provider archive file
* `insert` — Insert a provider into a provider archive file



## `wash par create`

Build a provider archive file

**Usage:** `wash par create [OPTIONS] --vendor <VENDOR> --name <NAME> --binary <BINARY>`

###### **Options:**

* `-v`, `--vendor <VENDOR>` — Vendor string to help identify the publisher of the provider (e.g. Redis, Cassandra, wasmcloud, etc). Not unique
* `-r`, `--revision <REVISION>` — Monotonically increasing revision number
* `--version <VERSION>` — Human friendly version string
* `-j`, `--schema <SCHEMA>` — Optional path to a JSON schema describing the link definition specification for this provider
* `-d`, `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (service). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-n`, `--name <NAME>` — Name of the capability provider
* `-a`, `--arch <ARCH>` — Architecture of provider binary in format ARCH-OS (e.g. x86_64-linux)

  Default value: `aarch64-macos`
* `-b`, `--binary <BINARY>` — Path to provider binary for populating the archive
* `--destination <DESTINATION>` — File output destination path
* `--compress` — Include a compressed provider archive
* `--disable-keygen` — Disables autogeneration of signing keys



## `wash par inspect`

Inspect a provider archive file

**Usage:** `wash par inspect [OPTIONS] <archive>`

###### **Arguments:**

* `<archive>` — Path to provider archive or OCI URL of provider archive

###### **Options:**

* `-d`, `--digest <DIGEST>` — Digest to verify artifact against (if OCI URL is provided for <archive>)
* `--allow-latest` — Allow latest artifact tags (if OCI URL is provided for <archive>)
* `-u`, `--user <USER>` — OCI username, if omitted anonymous authentication will be used
* `-p`, `--password <PASSWORD>` — OCI password, if omitted anonymous authentication will be used
* `--insecure` — Allow insecure (HTTP) registry connections
* `--insecure-skip-tls-verify` — Skip checking OCI registry's certificate for validity
* `--no-cache` — skip the local OCI cache



## `wash par insert`

Insert a provider into a provider archive file

**Usage:** `wash par insert [OPTIONS] --binary <BINARY> <archive>`

###### **Arguments:**

* `<archive>` — Path to provider archive

###### **Options:**

* `-a`, `--arch <ARCH>` — Architecture of binary in format ARCH-OS (e.g. x86_64-linux)

  Default value: `aarch64-macos`
* `-b`, `--binary <BINARY>` — Path to provider binary to insert into archive
* `-d`, `--directory <DIRECTORY>` — Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
* `-i`, `--issuer <ISSUER>` — Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `-s`, `--subject <SUBJECT>` — Path to subject seed key (service). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found
* `--disable-keygen` — Disables autogeneration of signing keys



## `wash plugin`

Manage wash plugins

**Usage:** `wash plugin <COMMAND>`

###### **Subcommands:**

* `install` — Install a wash plugin
* `uninstall` — Uninstall a plugin
* `list` — List installed plugins



## `wash plugin install`

Install a wash plugin

**Usage:** `wash plugin install [OPTIONS] <url>`

###### **Arguments:**

* `<url>` — URL of the plugin to install. Can be a file://, http://, https://, or oci:// URL

###### **Options:**

* `-u`, `--user <USER>` — OCI username, if omitted anonymous authentication will be used
* `-p`, `--password <PASSWORD>` — OCI password, if omitted anonymous authentication will be used
* `--insecure` — Allow insecure (HTTP) registry connections
* `--insecure-skip-tls-verify` — Skip checking server's certificate for validity
* `-d`, `--digest <DIGEST>` — Digest to verify plugin against. For OCI manifests, this is the digest format used in the manifest. For other types of plugins, this is the SHA256 digest of the plugin binary
* `--allow-latest` — Allow latest artifact tags (if pulling from OCI registry)
* `--update` — Whether or not to update the plugin if it is already installed. Defaults to false
* `--plugin-dir <PLUGIN_DIR>` — Path to plugin directory. Defaults to $HOME/.wash/plugins



## `wash plugin uninstall`

Uninstall a plugin

**Usage:** `wash plugin uninstall [OPTIONS] <id>`

###### **Arguments:**

* `<id>` — ID of the plugin to uninstall

###### **Options:**

* `--plugin-dir <PLUGIN_DIR>` — Path to plugin directory. Defaults to $HOME/.wash/plugins



## `wash plugin list`

List installed plugins

**Usage:** `wash plugin list [OPTIONS]`

###### **Options:**

* `--plugin-dir <PLUGIN_DIR>` — Path to plugin directory. Defaults to $HOME/.wash/plugins



## `wash push`

Push an artifact to an OCI compliant registry

**Usage:** `wash push [OPTIONS] <url> <artifact>`

###### **Arguments:**

* `<url>` — URL to push artifact to
* `<artifact>` — Path to artifact to push

###### **Options:**

* `-r`, `--registry <REGISTRY>` — Registry of artifact. This is only needed if the URL is not a full (OCI) artifact URL (ie, missing the registry fragment)
* `-c`, `--config <CONFIG>` — Path to config file, if omitted will default to a blank configuration
* `--allow-latest` — Allow latest artifact tags
* `-a`, `--annotation <annotations>` — Optional set of annotations to apply to the OCI artifact manifest
* `-u`, `--user <USER>` — OCI username, if omitted anonymous authentication will be used
* `-p`, `--password <PASSWORD>` — OCI password, if omitted anonymous authentication will be used
* `--insecure` — Allow insecure (HTTP) registry connections
* `--insecure-skip-tls-verify` — Skip checking server's certificate for validity



## `wash pull`

Pull an artifact from an OCI compliant registry

**Usage:** `wash pull [OPTIONS] <url>`

###### **Arguments:**

* `<url>` — URL of artifact

###### **Options:**

* `--destination <DESTINATION>` — File destination of artifact
* `-r`, `--registry <REGISTRY>` — Registry of artifact. This is only needed if the URL is not a full (OCI) artifact URL (ie, missing the registry fragment)
* `-d`, `--digest <DIGEST>` — Digest to verify artifact against
* `--allow-latest` — Allow latest artifact tags
* `-u`, `--user <USER>` — OCI username, if omitted anonymous authentication will be used
* `-p`, `--password <PASSWORD>` — OCI password, if omitted anonymous authentication will be used
* `--insecure` — Allow insecure (HTTP) registry connections
* `--insecure-skip-tls-verify` — Skip checking server's certificate for validity



## `wash secrets`

Manage secret references

**Usage:** `wash secrets <COMMAND>`

###### **Subcommands:**

* `put` — Put secret reference
* `get` — Get a secret reference by name
* `del` — Delete a secret reference by name



## `wash secrets put`

Put secret reference

**Usage:** `wash secrets put [OPTIONS] <name> <backend> <key>`

###### **Arguments:**

* `<name>` — The name of the secret reference to create
* `<backend>` — The backend to fetch the secret from at runtime
* `<key>` — The key to use for retrieving the secret from the backend

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--field <FIELD>` — The field to use for retrieving the secret from the backend
* `-v`, `--version <VERSION>` — The version of the secret to retrieve. If not supplied, the latest version will be used
* `--property <POLICY_PROPERTIES>` — Freeform policy properties to pass to the secrets backend, in the form of `key=value`. Can be specified multiple times



## `wash secrets get`

Get a secret reference by name

**Usage:** `wash secrets get [OPTIONS] <name>`

###### **Arguments:**

* `<name>`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash secrets del`

Delete a secret reference by name

**Usage:** `wash secrets del [OPTIONS] <name>`

###### **Arguments:**

* `<name>`

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash spy`

Spy on all invocations a component sends and receives

**Usage:** `wash spy [OPTIONS] <component_id>`

###### **Arguments:**

* `<component_id>` — Component ID to spy on, component or capability provider. This is the unique identifier supplied to the component at startup

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication



## `wash scale`

Scale a component running in a host to a certain level of concurrency

**Usage:** `wash scale <COMMAND>`

###### **Subcommands:**

* `component` — Scale a component running in a host to a certain level of concurrency



## `wash scale component`

Scale a component running in a host to a certain level of concurrency

**Usage:** `wash scale component [OPTIONS] <host-id> <component-ref> <component-id>`

###### **Arguments:**

* `<host-id>` — ID of host to scale component on. If a non-ID is provided, the host will be selected based on matching the friendly name and will return an error if more than one host matches
* `<component-ref>` — Component reference, e.g. the absolute file path or OCI URL
* `<component-id>` — Unique ID to use for the component

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `-c`, `--max-instances <MAX_INSTANCES>` — Maximum number of component instances allowed to run concurrently. Setting this value to `0` will stop the component

  Default value: `4294967295`
* `-a`, `--annotations <ANNOTATIONS>` — Optional set of annotations used to describe the nature of this component scale command. For example, autonomous agents may wish to “tag” scale requests as part of a given deployment
* `--config <CONFIG>` — List of named configuration to apply to the component, may be empty
* `--skip-wait` — By default, the command will wait until the component has been scaled. If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the component to be scaled. If this flag is omitted, the command will wait until the scaled event has been acknowledged
* `--wait-timeout-ms <WAIT_TIMEOUT_MS>` — Timeout for waiting for scale to occur (normally on an auction response), defaults to 2000 milliseconds

  Default value: `5000`



## `wash start`

Start a component or capability provider

**Usage:** `wash start <COMMAND>`

###### **Subcommands:**

* `component` — Launch a component in a host
* `provider` — Launch a provider in a host



## `wash start component`

Launch a component in a host

**Usage:** `wash start component [OPTIONS] <component-ref> <component-id>`

###### **Arguments:**

* `<component-ref>` — Component reference, e.g. the absolute file path or OCI URL
* `<component-id>` — Unique ID to use for the component

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--host-id <HOST_ID>` — Id of host or a string to match on the friendly name of a host. if omitted the component will be auctioned in the lattice to find a suitable host. If a string is supplied to match against, then the matching host ID will be used. If more than one host matches, then an error will be returned
* `--max-instances <MAX_INSTANCES>` — Maximum number of instances this component can run concurrently

  Default value: `1`
* `-c`, `--constraint <constraints>` — Constraints for component auction in the form of "label=value". If host-id is supplied, this list is ignored
* `--auction-timeout-ms <AUCTION_TIMEOUT_MS>` — Timeout to await an auction response, defaults to 2000 milliseconds

  Default value: `2000`
* `--skip-wait` — By default, the command will wait until the component has been started. If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the component to start. If this flag is omitted, the timeout will be adjusted to 5 seconds to account for component download times
* `--config <CONFIG>` — List of named configuration to apply to the component, may be empty



## `wash start provider`

Launch a provider in a host

**Usage:** `wash start provider [OPTIONS] <provider-ref> <provider-id>`

###### **Arguments:**

* `<provider-ref>` — Provider reference, e.g. the OCI URL for the provider
* `<provider-id>` — Unique provider ID to use for the provider

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--host-id <HOST_ID>` — Id of host or a string to match on the friendly name of a host. if omitted the provider will be auctioned in the lattice to find a suitable host. If a string is supplied to match against, then the matching host ID will be used. If more than one host matches, then an error will be returned
* `-l`, `--link-name <LINK_NAME>` — Link name of provider

  Default value: `default`
* `-c`, `--constraint <constraints>` — Constraints for provider auction in the form of "label=value". If host-id is supplied, this list is ignored
* `--auction-timeout-ms <AUCTION_TIMEOUT_MS>` — Timeout to await an auction response, defaults to 2000 milliseconds

  Default value: `2000`
* `--config <CONFIG>` — List of named configuration to apply to the provider, may be empty
* `--skip-wait` — By default, the command will wait until the provider has been started. If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the provider to start. If this flag is omitted, the timeout will be adjusted to 30 seconds to account for provider download times



## `wash stop`

Stop a component, capability provider, or host

**Usage:** `wash stop <COMMAND>`

###### **Subcommands:**

* `component` — Stop a component running in a host
* `provider` — Stop a provider running in a host
* `host` — Purge and stop a running host



## `wash stop component`

Stop a component running in a host

**Usage:** `wash stop component [OPTIONS] <component-id>`

###### **Arguments:**

* `<component-id>` — Unique component Id or a string to match on the prefix of the ID. If multiple components are matched, then an error will be returned with a list of all matching options

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--host-id <HOST_ID>` — Id of host to stop component on. If a non-ID is provided, the host will be selected based on matching the prefix of the ID or the friendly name and will return an error if more than one host matches. If no host ID is passed, a host will be selected based on whether or not the component is running on it. If more than 1 host is running this component, an error will be returned with a list of hosts running the component
* `--skip-wait` — By default, the command will wait until the component has been stopped. If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the component to stp[]



## `wash stop provider`

Stop a provider running in a host

**Usage:** `wash stop provider [OPTIONS] <provider-id>`

###### **Arguments:**

* `<provider-id>` — Provider Id (e.g. the public key for the provider) or a string to match on the prefix of the ID, or friendly name, or call alias of the provider. If multiple providers are matched, then an error will be returned with a list of all matching options

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--host-id <HOST_ID>` — Id of host to stop provider on. If a non-ID is provided, the host will be selected based on matching the prefix of the ID or the friendly name and will return an error if more than one host matches. If no host ID is passed, a host will be selected based on whether or not the provider is running on it. If more than 1 host is running this provider, an error will be returned with a list of hosts running the provider
* `--skip-wait` — By default, the command will wait until the provider has been stopped. If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the provider to stop



## `wash stop host`

Purge and stop a running host

**Usage:** `wash stop host [OPTIONS] <host-id>`

###### **Arguments:**

* `<host-id>` — Id of host to stop. If a non-ID is provided, the host will be selected based on matching the prefix of the ID or the friendly name and will return an error if more than one host matches

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--host-timeout <HOST_SHUTDOWN_TIMEOUT>` — The timeout in ms for how much time to give the host for graceful shutdown

  Default value: `2000`



## `wash label`

Label (or un-label) a host with a key=value label pair

**Usage:** `wash label [OPTIONS] <host-id> [label]...`

###### **Arguments:**

* `<host-id>` — ID of host to update the component on. If a non-ID is provided, the host will be selected based on matching the prefix of the ID or the friendly name and will return an error if more than one host matches
* `<label>` — Host label in the form of a `[key]=[value]` pair, e.g. "cloud=aws". When `--delete` is set, only the key is provided

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--delete` — Delete the label, instead of adding it

  Default value: `false`



## `wash update`

Update a component running in a host to newer image reference

**Usage:** `wash update <COMMAND>`

###### **Subcommands:**

* `component` — Update a component running in a host to a newer version



## `wash update component`

Update a component running in a host to a newer version

**Usage:** `wash update component [OPTIONS] <component-id> <new-component-ref>`

###### **Arguments:**

* `<component-id>` — Unique ID of the component to update
* `<new-component-ref>` — Component reference to replace the current running comonent with, e.g. the absolute file path or OCI URL

###### **Options:**

* `-r`, `--ctl-host <CTL_HOST>` — CTL Host for connection, defaults to 127.0.0.1 for local nats
* `-p`, `--ctl-port <CTL_PORT>` — CTL Port for connections, defaults to 4222 for local nats
* `--ctl-jwt <CTL_JWT>` — JWT file for CTL authentication. Must be supplied with ctl_seed
* `--ctl-seed <CTL_SEED>` — Seed file or literal for CTL authentication. Must be supplied with ctl_jwt
* `--ctl-credsfile <CTL_CREDSFILE>` — Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt. See https://docs.nats.io/using-nats/developer/connecting/creds for details
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — TLS CA file for CTL authentication. See https://docs.nats.io/using-nats/developer/connecting/tls for details
* `--js-domain <JS_DOMAIN>` — JS domain for wasmcloud control interface. Defaults to None
* `-x`, `--lattice <LATTICE>` — Lattice name for wasmcloud control interface, defaults to "default"
* `-t`, `--timeout-ms <TIMEOUT_MS>` — Timeout length to await a control interface response, defaults to 2000 milliseconds

  Default value: `2000`
* `--context <CONTEXT>` — Name of a context to use for CTL connection and authentication
* `--host-id <HOST_ID>` — ID of host to update the component on. If a non-ID is provided, the host will be selected based on matching the prefix of the ID or the friendly name and will return an error if more than one host matches. If no host ID is passed, a host will be selected based on whether or not the component is running on it. If more than 1 host is running this component, an error will be returned with a list of hosts running the component



## `wash up`

Bootstrap a wasmCloud environment

**Usage:** `wash up [OPTIONS]`

###### **Options:**

* `-d`, `--detached` — Launch NATS and wasmCloud detached from the current terminal as background processes
* `--nats-credsfile <NATS_CREDSFILE>` — Optional path to a NATS credentials file to authenticate and extend existing NATS infrastructure
* `--nats-config-file <NATS_CONFIGFILE>` — Optional path to a NATS config file NOTE: If your configuration changes the address or port to listen on from 0.0.0.0:4222, ensure you set --nats-host and --nats-port
* `--nats-remote-url <NATS_REMOTE_URL>` — Optional remote URL of existing NATS infrastructure to extend
* `--nats-connect-only` — If a connection can't be established, exit and don't start a NATS server. Will be ignored if a remote_url and credsfile are specified
* `--nats-version <NATS_VERSION>` — NATS server version to download, e.g. `v2.10.7`. See https://github.com/nats-io/nats-server/releases/ for releases

  Default value: `v2.10.7`
* `--nats-host <NATS_HOST>` — NATS server host to connect to
* `--nats-port <NATS_PORT>` — NATS server port to connect to. This will be used as the NATS listen port if `--nats-connect-only` isn't set
* `--nats-websocket-port <NATS_WEBSOCKET_PORT>` — NATS websocket port to use. TLS is not supported. This is required for the wash ui to connect from localhost

  Default value: `4223`
* `--nats-js-domain <NATS_JS_DOMAIN>` — NATS Server Jetstream domain for extending superclusters
* `--wasmcloud-version <WASMCLOUD_VERSION>` — wasmCloud host version to download, e.g. `v0.55.0`. See https://github.com/wasmCloud/wasmcloud/releases for releases

  Default value: `v1.1.0`
* `-x`, `--lattice <LATTICE>` — A unique identifier for a lattice, frequently used within NATS topics to isolate messages among different lattices
* `--host-seed <HOST_SEED>` — The seed key (a printable 256-bit Ed25519 private key) used by this host to generate it's public key
* `--rpc-host <RPC_HOST>` — An IP address or DNS name to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-host if not supplied
* `--rpc-port <RPC_PORT>` — A port to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-port if not supplied
* `--rpc-seed <RPC_SEED>` — A seed nkey to use to authenticate to NATS for RPC messages
* `--rpc-timeout-ms <RPC_TIMEOUT_MS>` — Timeout in milliseconds for all RPC calls

  Default value: `2000`
* `--rpc-jwt <RPC_JWT>` — A user JWT to use to authenticate to NATS for RPC messages
* `--rpc-tls` — Optional flag to enable host communication with a NATS server over TLS for RPC messages
* `--rpc-tls-ca-file <RPC_TLS_CA_FILE>` — A TLS CA file to use to authenticate to NATS for RPC messages
* `--rpc-credsfile <RPC_CREDSFILE>` — Convenience flag for RPC authentication, internally this parses the JWT and seed from the credsfile
* `--ctl-host <CTL_HOST>` — An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
* `--ctl-port <CTL_PORT>` — A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
* `--ctl-seed <CTL_SEED>` — A seed nkey to use to authenticate to NATS for CTL messages
* `--ctl-jwt <CTL_JWT>` — A user JWT to use to authenticate to NATS for CTL messages
* `--ctl-credsfile <CTL_CREDSFILE>` — Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
* `--ctl-tls` — Optional flag to enable host communication with a NATS server over TLS for CTL messages
* `--ctl-tls-ca-file <CTL_TLS_CA_FILE>` — A TLS CA file to use to authenticate to NATS for CTL messages
* `--cluster-seed <CLUSTER_SEED>` — The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
* `--cluster-issuers <CLUSTER_ISSUERS>` — A comma-delimited list of public keys that can be used as issuers on signed invocations
* `--provider-delay <PROVIDER_DELAY>` — Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process

  Default value: `300`
* `--allow-latest` — Determines whether OCI images tagged latest are allowed to be pulled from OCI registries and started
* `--allowed-insecure <ALLOWED_INSECURE>` — A comma-separated list of OCI hosts to which insecure (non-TLS) connections are allowed
* `--wasmcloud-js-domain <WASMCLOUD_JS_DOMAIN>` — Jetstream domain name, configures a host to properly connect to a NATS supercluster
* `--config-service-enabled` — Denotes if a wasmCloud host should issue requests to a config service on startup
* `--allow-file-load <ALLOW_FILE_LOAD>` — Denotes if a wasmCloud host should allow starting components from the file system

  Default value: `true`

  Possible values: `true`, `false`

* `--enable-structured-logging` — Enable JSON structured logging from the wasmCloud host
* `-l`, `--label <LABEL>` — A label to apply to the host, in the form of `key=value`. This flag can be repeated to supply multiple labels
* `--log-level <STRUCTURED_LOG_LEVEL>` — Controls the verbosity of JSON structured logs from the wasmCloud host

  Default value: `info`
* `--enable-ipv6` — Enables IPV6 addressing for wasmCloud hosts
* `--wasmcloud-start-only` — If enabled, wasmCloud will not be downloaded if it's not installed
* `--multi-local` — If enabled, allows starting additional wasmCloud hosts on this machine
* `--max-execution-time-ms <MAX_EXECUTION_TIME>` — Defines the Max Execution time (in ms) that the host runtime will execute for

  Default value: `600000`
* `--wadm-version <WADM_VERSION>` — wadm version to download, e.g. `v0.4.0`. See https://github.com/wasmCloud/wadm/releases for releases

  Default value: `v0.13.0`
* `--disable-wadm` — If enabled, wadm will not be downloaded or run as a part of the up command
* `--wadm-js-domain <WADM_JS_DOMAIN>` — The JetStream domain to use for wadm
* `--wadm-manifest <WADM_MANIFEST>` — The path to a wadm application manifest to run while the host is up



## `wash ui`

Serve a web UI for wasmCloud

**Usage:** `wash ui [OPTIONS]`

###### **Options:**

* `-p`, `--port <PORT>` — Which port to run the UI on, defaults to 3030

  Default value: `3030`
* `-v`, `--version <VERSION>` — Which version of the UI to run

  Default value: `v0.4.0`



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>

