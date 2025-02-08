#!/usr/bin/env bash

set -euo pipefail

source "$(dirname "$0")/common-env"

if [[ -z "${OCI_USER_ID:-}" ]] || [[ -z "${OCI_CREDENTIALS_FINGERPRINT:-}" ]] || [[ -z "${OCI_CREDENTIALS_KEY:-}" ]]; then
    echo "Error: Required environment variables are not set"
    echo "Please ensure OCI_USER_ID, OCI_CREDENTIALS_FINGERPRINT, and OCI_CREDENTIALS_KEY are set"
    exit 1
fi

# Get all the proper vars set
export OCI_TENANCY_ID_B64="$(echo -n "$OCI_TENANCY_ID" | base64 | tr -d '\n')"
export OCI_CREDENTIALS_FINGERPRINT_B64="$(echo -n "$OCI_CREDENTIALS_FINGERPRINT" | base64 | tr -d '\n')"
export OCI_USER_ID_B64="$(echo -n "$OCI_USER_ID" | base64 | tr -d '\n')"
export OCI_REGION_B64="$(echo -n "$OCI_REGION" | base64 | tr -d '\n')"
export OCI_CREDENTIALS_KEY_B64="$(echo -n "$OCI_CREDENTIALS_KEY" | base64 | tr -d '\n')"
export EXP_MACHINE_POOL=true

clusterctl init --infrastructure oci:v0.16.0
