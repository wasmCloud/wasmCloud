#!/usr/bin/env bash
# Makes the demo's API token and a CA for the two mock servers.
set -euo pipefail
cd "$(dirname "$0")/.."

mkdir -p .secrets mock/certs
umask 077
printf 'acme_live_%s\n' "$(openssl rand -hex 16)" > .secrets/acme-token

cd mock/certs
openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=egress-secrets demo CA" \
  -keyout ca.key -out ca.pem 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=host.wasmcloud.internal" \
  -keyout server.key -out server.csr 2>/dev/null
printf 'subjectAltName=DNS:host.wasmcloud.internal,DNS:localhost,IP:127.0.0.1\n' > san.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 30 \
  -extfile san.ext -out server.pem 2>/dev/null
rm -f server.csr san.ext ca.srl
echo "wrote .secrets/acme-token and mock/certs/"
