# Benchmarking and CI

This directory is mainly intended for use in our benchmarking chart and pipelines. However, it can also be used to manually run benchmarks as so desired.

Here is a brief overview of what this directory contains

- Shell scripts and helpers for running the benchmarking test
- A Dockerfile for a custom k6 image used in the benchmarking chart
- A `values.yaml` file for installing the benchmarking chart
- A directory of manifests used for configuring the cluster and resources for the benchmark
- A data aggregation script, written in python, for aggregating data from a distributed benchmarking test

These tests run weekly or on demand using capacity graciously donated to the CNCF by Oracle Cloud.

## Prerequisites

For running the benchmarking tests, you will need the following tools

- A running Kubernetes cluster (preferrably locally, though not required). This could be `kind` or the clusters provided with tools like Docker Desktop and Orbstack
- `kubectl`
- `helm`
- `jq`
- `wash`
- [`clusterctl`](https://cluster-api.sigs.k8s.io/user/quick-start#install-clusterctl)
- The [`oci` CLI](https://github.com/oracle/oci-cli#installation) for interacting with Oracle Cloud Infrastructure

You will also need to properly configure your local OCI CLI configuration file (recommended) using the automation credentials (talk to an Org Maintainer for access) or you can set the following environment variables:

- `OCI_CLI_KEY_CONTENT` - The private key used for authentication with the OCI API.
- `OCI_CLI_USER` - The OCID of the user to use with the OCI CLI.
- `OCI_CLI_FINGERPRINT` - The fingerprint of the key used for authentication with the OCI API.

For running the scripts, you need the following environment variables set:

- `OCI_USER_ID` - The OCID of the user to use with the OCI CLI.
- `OCI_CREDENTIALS_FINGERPRINT` - The fingerprint of the key used for authentication with the OCI API.
- `OCI_CREDENTIALS_KEY` - The private key used for authentication with the OCI API.

Note that these are similar environment variables used by the OCI CLI just with different names as they are specific to the script itself. You could technically use separate sets of credentials, hence the different variables

## Running the tests

For ease of use, these tests use Cluster API to provision a cluster in Oracle. Configuring a cluster with the plain `oci` CLI is burdensome and error prone, especially when it comes to cleanup. To run the tests:

1. Run the `./setup-cluster-api.sh` script to setup Cluster API for the tests
2. Run the `./run-benchmark.sh` script to run the tests. This script creates the cluster, then creates all of the resources for running wasmCloud, and then runs the benchmarks. On exit or error, it will automatically cleanup the cluster and resources so as to not waste credits
