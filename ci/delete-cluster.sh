#!/usr/bin/env bash

set -euo pipefail

source "$(dirname "$0")/common-env"

echo "Deleting cluster"

KUBERNETES_VERSION=v1.31.1 \
NAMESPACE=default \
clusterctl generate cluster benchmark-test \
--from "$(dirname "$0")/cluster-template-managed.yaml" | kubectl delete --wait=true --filename -
