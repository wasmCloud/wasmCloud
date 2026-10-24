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
caches its public key when building the connection options. Repeated auth
rejections close a bare-nkey connection and fail liveness so Kubernetes can
restart the operator with the new key.
`-nats-name` sets the connection name, `runtime-operator` by default.

### Helm Secret mounts

Set `operator.volumes` and `operator.volumeMounts` to mount an existing
Kubernetes Secret, then pass the matching file flag in `operator.extraArgs`.
For a Secret named `operator-nats-auth` with a `token` key:

```yaml
operator:
  extraArgs:
    - "-nats-token-file=/nats-auth/token"
  volumes:
    - name: nats-auth
      secret:
        secretName: operator-nats-auth
  volumeMounts:
    - name: nats-auth
      mountPath: /nats-auth
      readOnly: true
```

Mount the Secret directory without `subPath` so Kubernetes can refresh its
files. These mounts work with `global.tls.enabled` set to either value and
coexist with the chart's TLS certificate mount. The Secret must exist in the
operator's release namespace. Hosts need their own NATS credentials.

### Host NATS authentication

Hosts authenticate their scheduler and data connections separately, with the
same choice of one credential per connection:

| Flag (`scheduler` or `data`) | Credential |
| --- | --- |
| `--<plane>-nats-creds` | a `.creds` file (user JWT and nkey seed) |
| `--<plane>-nats-nkey` | a file holding an nkey seed |
| `--<plane>-nats-token-file` | a file holding a token |
| `--<plane>-nats-user`, `--<plane>-nats-password-file` | a user name, and a file holding its password |

Each flag also reads `WASH_<PLANE>_NATS_<NAME>`, such as
`WASH_SCHEDULER_NATS_CREDS`. Files are reread on every reconnect, and token
and password files lose one LF or CRLF line ending. While a file stalls, fails
to read, or does not parse, the host presents the last contents that parsed.

Give each connection its own credential, so the data connection can be denied
the control subjects under [Permissions](#permissions). Mount the Secret on a
host group and pass the flags in its `extraArgs`. The Secret must exist in the
host group's namespace:

```yaml
runtime:
  hostGroups:
    - name: default
      extraArgs:
        - "--scheduler-nats-creds=/nats-auth/scheduler.creds"
        - "--data-nats-creds=/nats-auth/data.creds"
      volumes:
        - name: nats-auth
          secret:
            secretName: host-nats-auth
      volumeMounts:
        - name: nats-auth
          mountPath: /nats-auth
          readOnly: true
```

The host refuses workload volumes that expose these files.

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
