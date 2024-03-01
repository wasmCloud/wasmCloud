## Overview

This directory contains examples to exercise various security measures in wasmCloud. You can read more about our zero trust security model and defense-in-depth measures in our [security documentation](https://wasmcloud.com/docs/hosts/security).

### Policy Server

wasmCloud supports pluggable policy servers over a simple NATS API, which is detailed in our [Prevent Untrusted Workloads](https://wasmcloud.com/docs/deployment/security/policy-service) documentation. The [opa](./opa) directory contains an example Go binary and instructions to run an [Open Policy Agent](https://www.openpolicyagent.org) server with a simple policy to ensure that every workload that runs in your wasmCloud lattice is signed by the official wasmCloud issuer.

This example is currently based off of `main`, so you'll need to build and run wasmCloud from source to see this in action. Once we cut a release candidate for our next release, this documentation will be updated.

Prerequisites:

- A Rust toolchain to run `wasmcloud`
- A Go toolchain to build the sample OPA server
- [nats-server](https://github.com/nats-io/nats-server) and [nats-cli](https://github.com/nats-io/nats)
- [jq](https://jqlang.github.io/jq/download/) for parsing JSON payloads

First, start NATS in the background, and run wasmCloud with a policy server configured (at the root of the repository):

```bash
nats-server -js &
WASMCLOUD_POLICY_TOPIC=wasmcloud.policy cargo run
```

Then, build the Go binary contained in [opa](./opa), or just run it directly:

```bash
> go run .
Listening for policy requests...
```

First, try starting a component that wasn't signed by wasmCloud:

```bash
export HOST_ID=$(nats req "wasmbus.ctl.default.host.ping" '{}' --raw | jq -r '.response.id')
nats req "wasmbus.ctl.default.actor.scale.$HOST_ID" '{"actor_id": "hello_world", "actor_ref": "ghcr.io/brooksmtownsend/http-hello-world-rust:0.1.0", "count": 1}'
```

You'll get a success message from the control interface telling you the request was received, but in the host logs you'll see:

```bash
2024-03-01T19:13:40.669666Z ERROR wasmcloud_host::wasmbus: failed to scale actor actor_ref=ghcr.io/brooksmtownsend/http-hello-world-rust:0.1.0 actor_id=hello_world err=Policy denied request to scale actor `a5e1deda-deb5-4b06-bc64-aa7bdcb9b3d7`: `None`
```

Now, try starting a provider that was signed by wasmCloud:

```bash
export HOST_ID=$(nats req "wasmbus.ctl.default.host.ping" '{}' --raw | jq -r '.response.id')
nats req "wasmbus.ctl.default.provider.start.$HOST_ID" '{"provider_id": "httpserver", "provider_ref": "wasmcloud.azurecr.io/httpserver:0.19.1"}'
```

```bash
2024-03-01T19:13:53.727940Z  INFO wasmcloud_host::wasmbus: handling start provider provider_ref="wasmcloud.azurecr.io/httpserver:0.19.1" provider_id="httpserver"
2024-03-01T19:13:54.965168Z  INFO wasmcloud_host::wasmbus: provider started provider_ref="wasmcloud.azurecr.io/httpserver:0.19.1" provider_id="httpserver"
Starting capability provider httpserver instance 4cf4db66-d679-4ed3-b00f-8410148e2b6f with nats url nats://127.0.0.1:4222
2024-03-01T19:13:55.066322Z  INFO wasmbus_rpc::rpc_client: nats client connected
```

Voila, the provider was allowed! For both components and providers, you are able to use the `wash` CLI to see if they are signed, and inspect their embedded claims using `wash inspect`. Taking a look at the component above, you can see that the issuer was not wasmCloud's official issuer. We're following the same logic in the policy to deny this component from starting.

```bash
➜ wash inspect ghcr.io/brooksmtownsend/http-hello-world-rust:0.1.0


                          http-hello-world - Actor
  Account         ACX4CGGHE3EGOI5HHJBNWXOVB3LVDDZL6YZOJTM4B42NGTBDXOH3DMTE
  Actor           MCCTSLNY5XCODQGV4EL2BJR56J7VDQADX5KYWZ3K2DCH7YHZRAEAQ763
  Expires                                                            never
  Can Be Used                                                  immediately
  Version                                                        0.1.0 (0)
  Call Alias                                                     (Not set)
                                Capabilities

                                    Tags
  wasmcloud.com/experimental

```
