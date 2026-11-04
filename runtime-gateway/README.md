# runtime-gateway

Distributes HTTP Requests to Wasm Hosts.

## How it works?

It watches both `Host` and `Workload` CRDs, creating an internal mapping of which workloads are associated with which hosts.

When an HTTP request is received, the gateway determines which host to forward the request to based on the request's hostname and the registered workloads. It then forwards the request to the appropriate host, which processes the request and returns a response.

It also enriches the request with additional headers with:

- `X-Real-Ip`: The IP address of the peer connected to the gateway. When a load balancer or ingress sits in front of the gateway, this is the proxy's address rather than the client's.
- `X-Workload-Id`: The unique identifier of the workload handling the request.

## Configuration

The gateway listens on `:8000` by default. Use `--bind-addr` to change the address.

Without `--fallback-endpoint`, requests without a matching workload receive a built-in 503 response. An external fallback must be an HTTP or HTTPS origin, such as `https://notfound.example:8443`, without credentials, a path, a query, or a fragment. A trailing `/` is allowed. Forwarded requests keep their original host, path, and query.

Use `--kubeconfig` to select a Kubernetes configuration file. Otherwise, the gateway uses controller-runtime's configuration discovery, including `KUBECONFIG` and in-cluster credentials.

Logs use JSON by default. `--dev-mode` and `--zap-devel` both enable development logging. The standard Zap flags, including `--zap-log-level`, `--zap-encoder`, and `--zap-time-encoding`, control logging output.
