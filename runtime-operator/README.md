# Runtime Operator

## NATS authentication

The operator and its hosts share one NATS control plane. On a NATS that
requires authentication, the operator presents one of:

| Flag | Credential |
| --- | --- |
| `-nats-creds` | a `.creds` file (user JWT and nkey seed) |
| `-nats-nkey` | a file holding an nkey seed |
| `-nats-token-file` | a file holding a token |
| `-nats-user`, `-nats-password-file` | a user name, and a file holding its password |

The operator refuses to start with more than one of these, and requires
`-nats-user` and `-nats-password-file` together. The NATS client would send
every credential it is given, and the server authenticates with only one.

Credentials, token, and password files are reread on every connect. When the
server rejects the operator's credential, as when the server rotates a token
before the kubelet refreshes the mounted Secret, the operator keeps
reconnecting with whatever the file holds until the server accepts it.
Token and password files may end with one LF or CRLF line ending, which is
removed. All other whitespace is preserved.
Restart the operator after replacing a bare nkey seed: the NATS client
caches its public key when building the connection options.
`-nats-name` sets the connection name, `runtime-operator` by default.

### Permissions

`<id>` is a host's id. The operator's replies arrive on its inbox prefix,
`_INBOX` unless `-nats-inbox-prefix` sets another.

| Client | Publish | Subscribe |
| --- | --- | --- |
| Operator | `runtime.host.>` | `runtime.operator.heartbeat.>`, `<inbox-prefix>.>` |
| Host, scheduler connection | `runtime.host.<id>.__control.ready`, `runtime.operator.heartbeat.<id>`, and replies (`allow_responses` with `max: 1`, or `<inbox-prefix>.>`) | `runtime.host.<id>.>` |

A host's scheduler connection must also receive its own messages, which is
the NATS default. Its data connection carries workload traffic and needs
none of the subjects above; deny it `runtime.>`.

Any client that may subscribe to `_INBOX.>` can read the replies hosts send
the operator, and any client that may publish there can forge one. On a NATS
shared with other clients, set `-nats-inbox-prefix` to a subject only the
operator may subscribe to, such as `runtime.operator.inbox`, and allow hosts
to reply there or by `allow_responses`.

A host id is chosen when the host process starts, so permissions that name
`<id>` need a credential issued per host process. With one credential per
host group, use `*` for `<id>`: those hosts can then read each other's
commands, though only the operator can send them.

## Development

You will need a local Kind cluster, NATS, and a Wash host.

For NATS, use:

```sh
docker run --rm --name wasmcloud-nats -p 4222:4222 nats -js
```

For Wash Host, use:

```sh
wash host --http-addr 127.0.0.1:8000
```

or from the top git directory:

```sh
cargo run -- host --http-addr 127.0.0.1:8000
```

For local kind cluster, refer to [wash/README.md](../README.md).

Install the CRDs into the cluster:

```sh
make install
```

Run the Builder / Watcher:

```sh
make devlog
```

You can apply development samples from the config/sample:

```sh
kubectl apply -f config/samples
```

Verify the Wash Host registered correctly with:

```sh
kubectl get host
```

Example:

```
❯ kubectl get host
NAME               HOSTID                                 HOSTGROUP   READY   AGE
exotic-toes-5866   14b0a1df-3f91-441c-b304-2aa56e9bef6e   default     True    52s
```

If you applied the samples, check if they deployed correctly:

```
❯ kubectl get workloaddeployment
NAME     REPLICAS   READY
blobby   1          True
hello    1          True

❯ curl -i hello.localhost.direct:8000
HTTP/1.1 200 OK
transfer-encoding: chunked
date: Tue, 25 Nov 2025 15:30:32 GMT

Hello from Rust!
```
