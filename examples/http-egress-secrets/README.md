# http-egress-secrets

An HTTP component that calls an API with a bearer token it never holds.

`wash dev` gives the component a placeholder in `API_TOKEN`. When the component
sends that placeholder as a bearer token to the one destination and route the
secret is bound to, the host writes the real token in its place on the way out.
Sent anywhere else, the request is refused before it leaves. So a compromised or
careless component can leak only the placeholder, and a placeholder is
worthless outside this host.

```
component ──Bearer wash_ph_…──▶ host ──Bearer acme_live_…──▶ Acme API
                                 │
                                 └─ any other host, route or header: denied
```

## Run it

You need `wash`, Rust with the `wasm32-wasip2` target, `python3` and `openssl`.

Make a demo token and a CA for the mock servers:

```bash
./mock/setup.sh
```

`wash dev` loads the CA when it starts, so restart it if you run this again.

In one terminal, start the mock Acme API (`:8443`) and a mock attacker
(`:9443`). Both log every credential they receive:

```bash
python3 -I mock/servers.py
```

In another, start the component:

```bash
wash dev
```

## Try it

| Request | What happens |
| --- | --- |
| `curl localhost:8000/env` | The component holds `wash_ph_…`, not the token. |
| `curl localhost:8000/whoami` | `200 OK`. The Acme API logs the real `acme_live_…` token. |
| `curl localhost:8000/echo` | The API echoes the token in a response header. The component sees the placeholder: the host swaps it back in response headers and trailers. |
| `curl localhost:8000/delete` | Denied. The secret is bound to `GET /v1` only. |
| `curl localhost:8000/exfil-header` | Denied. The secret is bound to the Acme API only, so the attacker logs nothing. |
| `curl localhost:8000/exfil-body` | Delivered. Bodies are not scanned, so the attacker receives the placeholder. Try it against the API yourself: `401`. |
| `./mock/rotate.sh`, then `/whoami` | The API now accepts only the new token, and the request still succeeds. `wash dev` rereads the file every second. The placeholder doesn't change. |

Replaying a leaked placeholder:

```bash
curl --cacert mock/certs/ca.pem -H "authorization: Bearer $(curl -s localhost:8000/env | cut -d= -f2)" \
  https://localhost:8443/v1/whoami
```

## How it is configured

`.wash/config.yaml` binds the secret:

```yaml
dev:
  secrets:
    - env: API_TOKEN                   # gets the placeholder
      fromFile: .secrets/acme-token    # or fromEnv: SOME_VAR
      destinations:
        - https://host.wasmcloud.internal:8443
      routes:
        - GET /v1
```

Without `header` or `query` the placeholder goes in `authorization` as a bearer
token; set `header: x-api-key` or `query: key` for APIs that want it elsewhere,
and `placeholderPrefix` for SDKs that check a key's shape. The secret goes only
over HTTPS, to exact hosts and ports.

The mock servers run on this machine, which a component reaches as
`host.wasmcloud.internal` on the ports in `allowedHostLoopbackPorts`. Against a
real API, list its host in `destinations` and `allowedHosts` and drop the
loopback ports and `http_client_ca_paths`.

Compare the top-level `secrets:` block that other examples use (for instance
[otel-config](../otel-config/)): that hands the real value to the component's
environment, where any code in it can read it.

## Outside `wash dev`

The component code doesn't change in production. A host given secrets by its
embedder substitutes them the same way, per workload, and can mint short-lived
credentials from the workload's own identity rather than reading a static
token.
