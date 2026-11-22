# External NATS

By default the chart installs a NATS server of its own (`nats.enabled: true`).
This example points the operator and the hosts at a NATS server that is
installed and managed somewhere else: a self-hosted NATS Helm release, a hosted
service, or a cluster another team runs. The chart then only installs the
operator and the hosts.

Two values files are included, one per authentication style:

| Values file | External NATS | Client authentication |
| --- | --- | --- |
| `values.external-nats-tls.yaml` | TLS | mTLS, certificates from the external server's CA |
| `values.external-nats-auth.yaml` | plaintext | username and password |

Both disable the deprecated Runtime Gateway the same way
`charts/runtime-operator/values.local.yaml` does, and install a single host
group with one replica.

## What the chart needs

The operator and every host dial the same control-plane NATS:
`global.nats.schedulerUrl` for the operator and the host scheduler connection,
`global.nats.dataUrl` for the data-plane connection workloads use. Both default
to the in-cluster Service of the bundled NATS, so pointing the chart at an
external server means setting both.

TLS client certificates and credentials are not templated per value. The chart
has one first-class knob, `global.tls.enabled`, which decides whether the
certificate Secrets are mounted and whether the `-nats-ca` and
`--scheduler-nats-tls-ca` family of flags is passed. Anything beyond that —
credentials, an externally issued certificate, a private CA — goes in through
`extraArgs`, `volumes` and `volumeMounts`, which every component and every host
group accepts.

## Scenario A: external NATS with TLS

The external server uses its own CA, so the chart must not generate
certificates. `values.external-nats-tls.yaml` therefore turns
`global.certificates.generate` off, which means the four Secrets the chart
would otherwise create have to exist before the install:

- `wasmcloud-operator-tls` — operator client certificate
- `wasmcloud-runtime-tls` — scheduler connection certificate for the hosts
- `wasmcloud-data-tls` — data-plane connection certificate for the hosts

Each is an opaque Secret with `tls.crt`, `tls.key` and `ca.crt`. It is a client
identity the external server has to accept, so the CA has to be one the server
trusts.

### 1. Install the external NATS

This step stands in for whatever runs the NATS server. It uses the official
NATS Helm chart with a self-signed CA.

```bash
helm repo add nats https://nats-io.github.io/k8s/helm/charts/
helm repo update nats
kubectl create namespace nats
```

Create the CA, the server certificate and one client certificate per identity:

```bash
mkdir -p /tmp/external-nats && cd /tmp/external-nats

openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
  -subj "/CN=external-nats-ca" -keyout ca.key -out ca.crt

openssl req -newkey rsa:2048 -nodes \
  -subj "/CN=nats.nats.svc.cluster.local" -keyout server.key -out server.csr
printf "subjectAltName=DNS:nats,DNS:nats.nats,DNS:nats.nats.svc,DNS:nats.nats.svc.cluster.local\n" > server.ext
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -days 2 -extfile server.ext -out server.crt

for identity in wasmcloud-operator wasmcloud-runtime wasmcloud-data; do
  openssl req -newkey rsa:2048 -nodes -subj "/CN=$identity" \
    -keyout "$identity.key" -out "$identity.csr"
  printf "subjectAltName=DNS:$identity\n" > "$identity.ext"
  openssl x509 -req -in "$identity.csr" -CA ca.crt -CAkey ca.key -CAcreateserial \
    -days 2 -extfile "$identity.ext" -out "$identity.crt"
done
```

Create the Secrets, one for the server and one per client identity:

```bash
kubectl create secret generic nats-server-tls -n nats \
  --from-file=tls.crt=server.crt --from-file=tls.key=server.key --from-file=ca.crt=ca.crt
kubectl create secret generic nats-ca-bundle -n nats --from-file=ca.crt=ca.crt

kubectl create namespace wasmcloud
for identity in wasmcloud-operator wasmcloud-runtime wasmcloud-data; do
  kubectl create secret generic "$identity-tls" -n wasmcloud \
    --from-file=tls.crt="$identity.crt" \
    --from-file=tls.key="$identity.key" \
    --from-file=ca.crt=ca.crt
done
```

Install the server with TLS required and JetStream on, and without a bundled CA
on the client side:

```bash
cat > /tmp/external-nats/nats-values.yaml <<'EOF'
config:
  jetstream:
    enabled: true
  cluster:
    enabled: false
  nats:
    tls:
      enabled: true
      secretName: nats-server-tls
tlsCA:
  enabled: true
  secretName: nats-ca-bundle
EOF

helm install nats nats/nats --namespace nats -f /tmp/external-nats/nats-values.yaml
```

### 2. Install the chart

```bash
kubectl apply --server-side -f charts/runtime-operator/crds/
helm upgrade --install wasmcloud charts/runtime-operator \
  --namespace wasmcloud -f deploy/external-nats/values.external-nats-tls.yaml
```

### 3. Verify

```bash
kubectl get pods -n wasmcloud
kubectl get host -n wasmcloud
```

Both pods reach `Running`, and the `Host` reaches `READY True`:

```text
NAME                                 READY   STATUS    RESTARTS   AGE
hostgroup-default-69955559d8-4vgkt   1/1     Running   0          80s
runtime-operator-7dcd69d8c-m4t5x     1/1     Running   0          80s

NAME                  HOSTID                                 HOSTGROUP   ENVIRONMENT   READY   AGE
typical-icicle-1895   01a20272-09fa-705e-9d7d-ee3925e7bf97   default     wasmcloud     True    46s
```

The connections show as TLS 1.3 in the server's monitoring endpoint:

```bash
kubectl exec -n nats nats-0 -c nats -- wget -qO- http://127.0.0.1:8222/connz
```

```text
conns 3
host:01a20272-09fa-705e-9d7d-ee3925e7bf97 |tls 1.3 |subs 1
host:01a20272-09fa-705e-9d7d-ee3925e7bf97:data |tls 1.3 |subs 0
runtime-operator |tls 1.3 |subs 2
```

## Scenario B: external NATS with username and password

`values.external-nats-auth.yaml` keeps TLS off and passes credentials through
mounts plus `extraArgs`. Passwords are given to the operator and the hosts as
files, so they never appear in the pod's command line.

### 1. Install the external NATS

```bash
helm repo add nats https://nats-io.github.io/k8s/helm/charts/
kubectl create namespace nats

cat > /tmp/external-nats/nats-auth-values.yaml <<'EOF'
config:
  jetstream:
    enabled: false
  cluster:
    enabled: false
  nats:
    tls:
      enabled: false
  merge:
    authorization:
      users:
        - { user: wasmcloud-operator, password: op-secret }
        - { user: wasmcloud-runtime, password: rt-secret }
        - { user: wasmcloud-data, password: dt-secret }
EOF

helm install nats nats/nats --namespace nats -f /tmp/external-nats/nats-auth-values.yaml
```

The three users are the ones both clients expect: the operator connects as
`wasmcloud-operator`, and each host opens a scheduler connection as
`wasmcloud-runtime` and a data-plane connection as `wasmcloud-data`.

### 2. Create the credential Secret and install the chart

```bash
kubectl create namespace wasmcloud
kubectl create secret generic wasmcloud-nats-auth -n wasmcloud \
  --from-literal=operator-password=op-secret \
  --from-literal=runtime-password=rt-secret \
  --from-literal=data-password=dt-secret

kubectl apply --server-side -f charts/runtime-operator/crds/
helm upgrade --install wasmcloud charts/runtime-operator \
  --namespace wasmcloud -f deploy/external-nats/values.external-nats-auth.yaml
```

### 3. Verify

```bash
kubectl get pods -n wasmcloud
kubectl get host -n wasmcloud
```

The host reaches `READY True` as in the TLS scenario. Authentication is
enforced by the server, so a client without credentials is refused while the
configured users connect:

```bash
kubectl exec -n nats deploy/nats-box -- \
  nats --server nats://nats:4222 --timeout 3s pub test.subject hi
# nats: error: nats: Authorization Violation

kubectl exec -n nats deploy/nats-box -- \
  nats --server nats://wasmcloud-operator:op-secret@nats:4222 --timeout 3s pub test.subject hi
# Published 2 bytes to "test.subject"
```

## Notes

- **Do not leave certificate generation on against an external TLS server.**
  With `global.tls.enabled: true` and the default `certificates.generate: true`,
  the chart mints its own CA and signs the client certificates with it. The
  external server does not trust that CA, so it refuses every handshake
  (`tls: bad certificate` for the operator, `tls: unknown certificate authority`
  for the hosts) and neither the operator nor a host ever connects. Either
  supply certificates from the server's CA as above, or keep the chart's CA and
  have the external server trust it.
- **JetStream is not required by the control plane.** With the credentials
  scenario above and JetStream disabled on the server, the operator connects,
  the host registers and the `Host` reports `READY True`. Workloads that use the
  `wasmcloud:nats` plugin's JetStream or KV bindings still need a server with
  JetStream enabled.
- **NetworkPolicy does not need a change.** The chart leaves egress
  unrestricted, so a host or the operator can reach a NATS cluster in another
  namespace or outside the cluster. There is no ingress rule for NATS because
  the external server is not installed by this chart.
- **The gateway stays off.** Routing is handled by the operator through
  EndpointSlices; this matches `values.local.yaml`.

## Verified with

`kind` v0.30.0 (Kubernetes v1.34.0), Helm v3.16.3, `runtime-operator` chart
2.12.3 (appVersion 2.12.3), NATS Helm chart 2.15.3, on macOS arm64.
