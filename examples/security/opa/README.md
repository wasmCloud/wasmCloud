# Sample Policy Server

This Go program is a sample policy server for wasmCloud. It uses [NATS](https://nats.io/) to interact with wasmCloud over its pluggable policy service API, [OPA](https://www.openpolicyagent.org) as a policy engine, and a simple [policy.rego](./policy.rego) rule which ensures that every component or provider started in a wasmCloud lattice is signed by the official wasmCloud issuer key.

You can read more about the policy service API [in our documentation](https://wasmcloud.com/docs/deployment/security/policy-service).

## Running the Policy Server

Simply use `go run .` to build and run the policy server once you've started a NATS server that is configured to listen on `localhost:4222`.
