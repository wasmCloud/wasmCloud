#!/usr/bin/env bash
set -euo pipefail

while true; do
  echo "checking for svc endpoints..."
  endpoints="$(kubectl -n testbed get endpoints nats -o jsonpath='{ .subsets }')"
  [ -n "${endpoints}" ] && break
  sleep 1
done

echo "NATS Proxy starting at localhost:4222"
exec kubectl -n testbed port-forward svc/nats 4222:4222
