# wcc
waSCC Controller - A single CLI to handle all of your waSCC tooling needs

## Using wcc
```
wcc <subcommand> [args]
```

## Subcommands

### claims

```
USAGE:
    wcc claims token <keytype>

FLAGS:
    -h, --help    Prints help information

ARGS:
    <tokentype>    The type of jwt to generate. May be Account, Actor, or Operator.
```

```
USAGE:
    wcc claims inspect [FLAGS] <file>

FLAGS:
    -h, --help    Prints help information
    -r, --raw     Extract the raw JWT from the file and print to stdout

ARGS:
    <file>    The WASM file to inspect
```

### gantry

```
USAGE:
    wcc gantry <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    download    Downloads an actor module from the registry
    get         Query the Gantry registry
    help        Prints this message or the help of the given subcommand(s)
    put         Puts a token in the registry
    upload      Uploads an actor module to the registry
```

### keys

```
USAGE:
    wcc keys gen <keytype>

FLAGS:
    -h, --help    Prints help information

ARGS:
    <keytype>    The type of key pair to generate. May be Account, User, Module (Actor), Server, Operator, Cluster, Service (Capability Provider)
```

### lattice

```
USAGE:
    wcc lattice [FLAGS] [OPTIONS] <SUBCOMMAND>

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
    help     Prints this message or the help of the given subcommand(s)
    list     List entities of various types within the lattice
    start    Hold a lattice auction for a given actor and start it if a suitable host is found
    stop     Tell a given host to terminate the given actor
    watch    Watch events on the lattice
```

### sign

```
USAGE:
    wcc sign [FLAGS] [OPTIONS] <source> <output> --issuer <issuer-key-path> --name <name> --subject <subject-key-path>

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
    -p, --prov           Indicates whether the signed module is a capability provider instead of an actor (the default
                         is actor)

OPTIONS:
    -c, --cap <capabilities>...         Add custom capabilities
    -x, --expires <expires-in-days>     Indicates the token expires in the given amount of days. If this option is left
                                        off, the token will never expire
    -i, --issuer <issuer-key-path>      Issuer seed key path (usually a .nk file)
    -n, --name <name>                   A human-readable, descriptive name for the token
    -b, --nbf <not-before-days>         Period in days that must elapse before this token is valid. If this option is
                                        left off, the token will be valid immediately
    -r, --rev <rev>                     Revision number
    -u, --subject <subject-key-path>    Subject seed key path (usually a .nk file)
    -t, --tag <tags>...                 A list of arbitrary tags to be embedded in the token
    -v, --ver <ver>                     Human-readable version string

ARGS:
    <source>    File to read
    <output>    Target output file
```
