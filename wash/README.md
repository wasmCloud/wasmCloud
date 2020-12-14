# wash
wasmCloud Shell - A single CLI to handle all of your wasmCloud tooling needs

## Installing wash
```
cargo install wash-cli
```

## Using wash
```
wash <subcommand> [args]
```

## Subcommands

### claims

```
USAGE:
    wash claims inspect [FLAGS] <file>

FLAGS:
    -h, --help    Prints help information
    -r, --raw     Extract the raw JWT from the file and print to stdout

ARGS:
    <file>    The WASM file to inspect
```

```
USAGE:
    wash claims sign [FLAGS] [OPTIONS] <module> <output> --name <name>

FLAGS:
    -f, --blob_store     Enable access to the blob store capability
    -e, --events         Enable access to an append-only event stream provider
    -z, --extras         Enable access to the extras functionality (random nos, guids, etc)
        --help           Prints help information
    -h, --http_client    Enable the HTTP client standard capability
    -s, --http_server    Enable the HTTP server standard capability
    -k, --keyvalue       Enable the Key/Value Store standard capability
    -l, --logging        Enable access to logging capability
    -g, --msg            Enable the Message broker standard capability
    -p, --prov           Indicates whether the signed module is a capability provider instead of an actor (the default is actor)

OPTIONS:
    -c, --cap <capabilities>...         Add custom capabilities
    -x, --expires <expires-in-days>     Indicates the token expires in the given amount of days. If this option is left
                                        off, the token will never expire
    -i, --issuer <issuer-key-path>      Issuer seed key path (usually a .nk file). If this option is left off, `wash` will attempt to locate an account key at `$HOME/.wash/keys/<module>_account.nk`, and if it is not found then an issuer key will be generated and placed in `$HOME/.wash/keys/<module>_account.nk`. You can also override this directory by setting the `WASH_KEYS` environment variable.
    -n, --name <name>                   A human-readable, descriptive name for the token
    -b, --nbf <not-before-days>         Period in days that must elapse before this token is valid. If this option is
                                        left off, the token will be valid immediately
    -r, --rev <rev>                     Revision number
    -u, --subject <subject-key-path>    Subject seed key path (usually a .nk file). If this option is left off, `wash` will attempt to locate a module key at `$HOME/.wash/keys/<module>_module.nk`, and if it is not found then a module key will be generated and placed in `$HOME/.wash/keys/<module>_module.nk`. You can also override this directory by setting the `WASH_KEYS` environment variable.
    -t, --tag <tags>...                 A list of arbitrary tags to be embedded in the token
    -v, --ver <ver>                     Human-readable version string

ARGS:
    <module>    WASM to read
    <output>    Target output file. Defaults to `<module_location>/<module>_signed.wasm`
```

```
USAGE:
    wash claims token <tokentype>

FLAGS:
    -h, --help    Prints help information

SUBCOMMANDS:
    account     Generate a signed JWT for an account
    actor       Generate a signed JWT for an actor module
    operator    Generate a signed JWT for an operator
```

### keys

```
USAGE:
    wash keys gen <keytype>

FLAGS:
    -h, --help    Prints help information

ARGS:
    <keytype>    The type of keypair to generate. May be Account, User, Module (Actor), Server, Operator, Cluster, Service (Capability Provider)
```

```
USAGE:
    wash keys get [OPTIONS] <keyname>

FLAGS:
    -h, --help      Prints help information

OPTIONS:
    -d, --directory <keysdirectory>     The directory where keys are stored for listing. Defaults to `$HOME/.wash/keys`, and can also be overwritten by setting the WASH_KEYS environment variable.

ARGS:
    <keyname>   The name of the key to output
```

```
USAGE:
    wash keys list [OPTIONS]

FLAGS:
    -h, --help          Prints help information

OPTIONS:
    -d, --directory <keysdirectory>     The directory where keys are stored for listing. Defaults to `$HOME/.wash/keys`, and can also be overwritten by setting the WASH_KEYS environment variable.
```

### lattice

```
USAGE:
    wash lattice [FLAGS] [OPTIONS] <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -j, --json       Render the output in JSON (if the command supports it)
    -V, --version    Prints version information

OPTIONS:
    -t, --timeout <call-timeout>    Lattice invocation / request timeout period, in milliseconds [env:
                                    LATTICE_RPC_TIMEOUT_MILLIS]  [default: 600]
    -c, --creds <creds>             Credentials file used to authenticate against NATS [env: LATTICE_CREDS_FILE]
    -n, --namespace <namespace>     Lattice namespace [env: LATTICE_NAMESPACE]
    -u, --url <url>                 The host IP of the nearest NATS server/leaf node to connect to the lattice [env:
                                    LATTICE_HOST]  [default: 127.0.0.1]

SUBCOMMANDS:
    list     List entities of various types within the lattice
    start    Hold a lattice auction for a given actor and start it if a suitable host is found
    stop     Tell a given host to terminate the given actor
    watch    Watch events on the lattice
```

### par

```
USAGE:
    wash par <SUBCOMMAND> [FLAGS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    create      Build a provider archive file
    insert      Insert a provider into a provider archive file
    inspect     Inspect a provider archive file
```

### reg

```
USAGE:
    wash reg <SUBCOMMAND> <artifact> [FLAGS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    pull        Downloads a blob from an OCI compliant registry
    push        Uploads a blob to an OCI compliant registry

ARGS:
    <artifact>       URI of the artifact
```

### up
Starts an interactive REPL session for wasmCloud development
```
USAGE:
    wash up [FLAGS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    help        Prints this message or the help of the given subcommand(s)
```
