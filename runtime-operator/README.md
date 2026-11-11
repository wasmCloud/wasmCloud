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
If the server is still refusing it after `-nats-auth-failure-grace` (five
minutes by default, longer than the kubelet takes to refresh a Secret),
liveness fails so Kubernetes restarts the operator and the refusal shows.
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

`--<plane>-nats-jwt-as-token` sends a `.creds` file's user JWT a second time,
as the token. A NATS server without trusted operators drops the JWT before
an auth callout sees it, so a callout that authorizes by JWT reads this copy.

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

`<id>` is a host's id. A heartbeat's payload ID must match its subject's
ID. An existing Host name cannot be claimed by a different ID; remove the
old Host before reusing its name with a new ID. These checks rely on NATS
permissions restricting each host to its own heartbeat subject.

The operator's replies arrive on its inbox prefix,
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

Set `wash host --host-id <id>` or `WASH_HOST_ID` to match a credential's
permitted host ID. IDs must contain 1–128 ASCII letters, digits, `_` or `-`
and must be unique among running hosts. The default remains a random UUID
chosen at startup, which needs a credential issued for that ID. With one
credential per host group, use `*` for `<id>`: those hosts can then read
each other's commands and publish each other's heartbeats, though only the
operator can send commands.

## Logging

The operator and the gateway both configure Go's `log/slog` and controller-runtime's
logr bridge through [`pkg/logging`](pkg/logging), so they accept the same flags. Logs go
to stderr. Development mode defaults to text output, debug logging (controller
verbosity 1), RFC3339 timestamps, and stack traces at warn and above. Use
`--dev-mode=false` for JSON output, info logging, and stack traces at error and
above. Text output uses slog's key=value format; JSON keeps `ts`, `level`, `msg`,
`logger`, `error`, and `stacktrace` fields. User fields named `time` keep their
name. Production logging is unsampled.

Kubernetes object pointers, passed directly or through `logr.Marshaler`, are
logged as identity in every mode, including the default Helm deployment. Full
contents, including Secret data, require an explicit `--log-full-objects` flag.
Objects passed by value, or nested in slices, maps, or other structs, are not
redacted.

| Flag | Values |
| --- | --- |
| `--log-full-objects` | Include full Kubernetes objects; defaults to false |
| `--log-format` | `text`, `json` |
| `--log-level` | `debug`, `info`, `warn`, `error`, `panic`, or verbosity `1`–`127` |
| `--log-stacktrace-level` | `info`, `warn`, `error`, `panic` |
| `--log-time-encoding` | `epoch`, `millis`, `nanos`, `iso8601`, `rfc3339`, `rfc3339nano` |

Explicit logging flags override mode defaults regardless of argument order.
`--json-log` overrides format and timestamp flags with JSON and numeric epoch
seconds. The existing `--zap-devel`, `--zap-encoder`, `--zap-log-level`,
`--zap-stacktrace-level`, and `--zap-time-encoding` flags remain aliases;
`--zap-encoder=console` selects text output. If an alias and its new flag both
appear, the last one wins. `panic` sets a threshold above error; logging does
not itself panic. Controller records include a numeric `v` field. Standard
library HTTP server diagnostics are logged at error without application stack
traces. Application stack traces start at the log call. In text output a stack
trace is a single escaped `stacktrace="…\n\t…"` value.

## Host cleanup

The `workload-host` controller reconciles live Hosts. Deleting Hosts are handled
by `workload-host-cleanup`, whose two workers share a limit of 24 concurrent
Workload deletes. Cleanup has its own queue so overlapping Host removals cannot
occupy the workers that make replacement Hosts Ready.

A cleanup error retains the Host finalizer and retries through the cleanup
queue. Cleanup also resumes for deleting Hosts after an operator restart.
Deletes check object identity and resource version; a stale Host deletion
decision must be reconciled again against the current Host.
Heartbeat status writes also check resource version so an old probe cannot
overwrite a replacement Host's status.

## Scaling and cleanup

Workload cleanup runs in its own controller, so slow stops do not occupy the
workers that place and monitor live workloads. Both queues yield when a host
has reached its concurrency limit. A shared per-object guard prevents a start
and finalization from overlapping, and placement checks current API state before
sending Start. Host selection remains a durable status checkpoint before that RPC.

The standalone operator exposes these bounds through flags; embedded callers
can set the corresponding fields on `EmbeddedOperatorConfig`:

| Flag | Default | Purpose |
| --- | --- | --- |
| `--workload-concurrency` | 8 | Live Workload workers |
| `--workload-cleanup-concurrency` | 16 | Workload cleanup workers |
| `--workload-host-concurrency` | 2 | Concurrent reconciliations per host, separately in each queue |
| `--replica-batch-size` | 64 | Creates or deletes per ReplicaSet scaling or cleanup batch |
| `--replica-concurrency` | 8 | Concurrent API mutations within a ReplicaSet batch |
| `--ready-requeue-interval` | 0 | Healthy-object polling interval; zero retains controller defaults |

ReplicaSet scaling rechecks the parent through an uncached read before each
batch. Deletes retain UID and resourceVersion preconditions. Scale-down retains
a deterministic prefix of live children and counts terminating children as
already selected, so retries do not choose extra victims. Operator-created
Workloads carry their cleanup finalizer from creation. Failed stops on Ready
hosts retain the finalizer and back off together. After a failed stop, queued
cleanup on a missing or explicitly failed Host can use the existing unavailable-host
cleanup policy without repeating the timeout; suppression alone never authorizes
that shortcut for a Ready or Unknown Host.

Child and route events coalesce over 100 milliseconds without extending the
deadline when new events arrive. Routes omit terminating workloads, resolve
each host once per pass, and use EndpointSlices of at most 100 endpoints.
Existing slice membership is preserved when endpoints disappear.

Hosts supporting `workload.status.batch` answer up to 128 status queries in one
RPC. The operator groups assignments with a local host/bucket index and reuses
observations for at most 30 seconds. Observations predating placement are ignored;
missing or mismatched response entries invalidate the batch. These are bounded
request/reply observations, not an atomic fleet snapshot. Operator restart drops
the cache and rebuilds it from fresh queries. Hosts that do not answer, or return
an invalid response, use the existing individual status RPC; batch capability is
retried after one minute. An unsupported batch probe can add up to five seconds
before fallback; sharing observations can add up to 30 seconds to failure
detection beyond the configured polling interval. Individual start/stop
confirmations remain uncached.

The runtime applies the normal status handler to every batch entry, including
custom authorization. Requests are limited to 128 IDs of at most 128 bytes each;
diagnostic messages in batch replies are truncated to 1 KiB at a UTF-8 boundary.
The existing query admission limit also applies to batch requests.

Use controller-runtime queue/worker metrics and API-server request metrics to
tune these limits. Measure traffic withdrawal, actual runtime cleanup, and
Kubernetes object removal separately: a successful stop acknowledgement can
precede the runtime's background cleanup. Increasing concurrency does not remove
the per-object API writes or establish capacity for 100,000 workloads.

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
