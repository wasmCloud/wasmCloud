#!/usr/bin/env bash
# Issues a new API token. The mock API accepts only the new one, and wash dev
# picks it up within a second.
set -euo pipefail
cd "$(dirname "$0")/.."
umask 077
printf 'acme_live_%s\n' "$(openssl rand -hex 16)" > .secrets/acme-token.next
mv .secrets/acme-token.next .secrets/acme-token
echo "rotated .secrets/acme-token"
