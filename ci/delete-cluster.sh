#!/usr/bin/env bash

set -euo pipefail

source "$(dirname "$0")/common-env"

kubectl delete machinepools --all --wait=true || echo "Machine pools already deleted"
kubectl delete clusters --all --wait=true || echo "Clusters already deleted"

echo "Deleting volumes"

# The deletion process for the cluster doesn't clean up block volumes (yay) so we do that here
oci bv volume list --output json --compartment-id "$OCI_COMPARTMENT_ID" | jq -r '.data[].id' | xargs -I {} oci bv volume delete --force --volume-id {} || echo "Volumes already deleted"
